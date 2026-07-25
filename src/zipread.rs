//! ZIP reading: structure parsing, decryption, parallel decompression.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use memmap2::Mmap;
use rayon::prelude::*;
use zune_inflate::{DeflateDecoder, DeflateOptions};

use crate::cli::{Mode, Options, Overwrite};
use crate::common::{self, fmt_size, Progress};
use crate::crypto::{self, CryptoError};
use crate::safepath::{self, Reject};

const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_EOCD64_LOC: u32 = 0x0706_4b50;
const SIG_EOCD64: u32 = 0x0606_4b50;
const SIG_CDIR: u32 = 0x0201_4b50;
const SIG_LOCAL: u32 = 0x0403_4b50;

/// Above this size an entry streams to disk instead of buffering in RAM.
const STREAM_THRESHOLD: u64 = 32 * 1024 * 1024;
const IO_CHUNK: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub struct AesInfo {
    pub vendor_version: u16,
    pub strength: u8,
    pub real_method: u16,
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc: u32,
    pub csize: u64,
    pub usize_: u64,
    pub local_off: u64,
    pub dos_time: u16,
    pub dos_date: u16,
    pub is_dir: bool,
    pub aes: Option<AesInfo>,
}

impl Entry {
    pub fn encrypted(&self) -> bool {
        self.flags & 1 != 0
    }

    /// The real compression method; for AES it lives in the extra field.
    pub fn real_method(&self) -> u16 {
        match &self.aes {
            Some(a) => a.real_method,
            None => self.method,
        }
    }

    pub fn method_name(&self) -> &'static str {
        match self.real_method() {
            0 => "Store",
            8 => "Deflate",
            9 => "Deflate64",
            12 => "BZip2",
            14 => "LZMA",
            93 => "Zstd",
            95 => "XZ",
            96 => "JPEG",
            98 => "PPMd",
            _ => "?",
        }
    }

    pub fn enc_name(&self) -> &'static str {
        if !self.encrypted() {
            ""
        } else if let Some(a) = &self.aes {
            match a.strength {
                1 => "AES128",
                2 => "AES192",
                _ => "AES256",
            }
        } else {
            "ZipCrypto"
        }
    }
}

// ---------------- little-endian readers ----------------

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7],
    ])
}

// ---------------- structure parsing ----------------

fn find_eocd(data: &[u8]) -> Option<usize> {
    let n = data.len();
    if n < 22 {
        return None;
    }
    let scan_start = n.saturating_sub(22 + 65_535);
    let mut i = n - 22;
    loop {
        if u32le(data, i) == SIG_EOCD {
            let clen = u16le(data, i + 20) as usize;
            if i + 22 + clen <= n {
                return Some(i);
            }
        }
        if i == scan_start {
            return None;
        }
        i -= 1;
    }
}

fn locate_central_dir(data: &[u8]) -> Result<(u64, u64), String> {
    let eocd = find_eocd(data)
        .ok_or_else(|| "not a valid ZIP file (no end-of-archive record found)".to_string())?;

    let mut count = u16le(data, eocd + 10) as u64;
    let mut cd_off = u32le(data, eocd + 16) as u64;

    let need64 = count == 0xFFFF || cd_off == 0xFFFF_FFFF;
    if eocd >= 20 && u32le(data, eocd - 20) == SIG_EOCD64_LOC {
        // Offsets here are attacker-controlled. Compare in u64 so a hostile
        // value cannot wrap the addition and slip past the bounds check.
        let e64_off = u64le(data, eocd - 20 + 8);
        if e64_off.saturating_add(56) <= data.len() as u64
            && u32le(data, e64_off as usize) == SIG_EOCD64
        {
            let e64 = e64_off as usize;
            count = u64le(data, e64 + 32);
            cd_off = u64le(data, e64 + 48);
        } else if need64 {
            return Err("corrupt ZIP64 file (bad end-of-archive location)".to_string());
        }
    } else if need64 {
        return Err("corrupt ZIP64 file (missing ZIP64 locator)".to_string());
    }

    if cd_off.saturating_add(22) > data.len() as u64 {
        return Err("corrupt archive (central directory lies outside the file)".to_string());
    }

    Ok((cd_off, count))
}

pub fn parse(data: &[u8]) -> Result<Vec<Entry>, String> {
    let (cd_off, count) = locate_central_dir(data)?;
    // A central directory record is at least 46 bytes, so the file size is a
    // hard upper bound on the entry count. Reserving from the declared count
    // alone would let a tiny hostile archive demand a huge allocation.
    let plausible = (data.len() / 46 + 1) as u64;
    let mut entries = Vec::with_capacity(count.min(plausible) as usize);
    let mut p = cd_off as usize;

    for idx in 0..count {
        if p.saturating_add(46) > data.len() || u32le(data, p) != SIG_CDIR {
            return Err(format!("corrupt central directory at entry {}", idx));
        }
        let flags = u16le(data, p + 8);
        let method = u16le(data, p + 10);
        let dos_time = u16le(data, p + 12);
        let dos_date = u16le(data, p + 14);
        let crc = u32le(data, p + 16);
        let mut csize = u32le(data, p + 20) as u64;
        let mut usize_ = u32le(data, p + 24) as u64;
        let name_len = u16le(data, p + 28) as usize;
        let extra_len = u16le(data, p + 30) as usize;
        let comment_len = u16le(data, p + 32) as usize;
        let ext_attrs = u32le(data, p + 38);
        let mut local_off = u32le(data, p + 42) as u64;

        if p + 46 + name_len + extra_len + comment_len > data.len() {
            return Err(format!("corrupt central directory at entry {}", idx));
        }
        let name_raw = &data[p + 46..p + 46 + name_len];
        let extra = &data[p + 46 + name_len..p + 46 + name_len + extra_len];

        let mut aes: Option<AesInfo> = None;
        let mut q = 0usize;
        while q + 4 <= extra.len() {
            let id = u16le(extra, q);
            let sz = u16le(extra, q + 2) as usize;
            if q + 4 + sz > extra.len() {
                break;
            }
            match id {
                0x0001 => {
                    let mut r = q + 4;
                    if usize_ == 0xFFFF_FFFF && r + 8 <= q + 4 + sz {
                        usize_ = u64le(extra, r);
                        r += 8;
                    }
                    if csize == 0xFFFF_FFFF && r + 8 <= q + 4 + sz {
                        csize = u64le(extra, r);
                        r += 8;
                    }
                    if local_off == 0xFFFF_FFFF && r + 8 <= q + 4 + sz {
                        local_off = u64le(extra, r);
                    }
                }
                0x9901 if sz >= 7 => {
                    aes = Some(AesInfo {
                        vendor_version: u16le(extra, q + 4),
                        strength: extra[q + 8],
                        real_method: u16le(extra, q + 9),
                    });
                }
                _ => {}
            }
            q += 4 + sz;
        }

        let name = decode_name(name_raw, flags & (1 << 11) != 0);
        // A directory entry may end in either separator. The spec says `/`, but
        // plenty of Windows producers write `\`, and treating one of those as a
        // file makes fzip try to create a file over an existing directory —
        // which fails with "Access is denied" and loses the whole subtree.
        let is_dir = name.ends_with('/')
            || name.ends_with('\\')
            || ((ext_attrs & 0x10) != 0 && usize_ == 0);

        entries.push(Entry {
            name, method, flags, crc, csize, usize_, local_off,
            dos_time, dos_date, is_dir, aes,
        });

        p += 46 + name_len + extra_len + comment_len;
    }

    Ok(entries)
}

fn data_start(data: &[u8], e: &Entry) -> Result<usize, String> {
    // local_off comes straight from the archive. Do every comparison in u64 so
    // a crafted 0xFFFF_FFFF_FFFF_FFF0 cannot wrap and index past the mapping.
    if e.local_off.saturating_add(30) > data.len() as u64 {
        return Err(format!("{}: corrupt local header", e.name));
    }
    let off = e.local_off as usize;
    if u32le(data, off) != SIG_LOCAL {
        return Err(format!("{}: corrupt local header", e.name));
    }
    let name_len = u16le(data, off + 26) as u64;
    let extra_len = u16le(data, off + 28) as u64;
    let start = e.local_off + 30 + name_len + extra_len;
    if start.saturating_add(e.csize) > data.len() as u64 {
        return Err(format!("{}: data extends past end of file", e.name));
    }
    Ok(start as usize)
}

// ---------------- entry names ----------------

#[rustfmt::skip]
const CP437_HIGH: [char; 128] = [
    'Ç','ü','é','â','ä','à','å','ç','ê','ë','è','ï','î','ì','Ä','Å',
    'É','æ','Æ','ô','ö','ò','û','ù','ÿ','Ö','Ü','¢','£','¥','₧','ƒ',
    'á','í','ó','ú','ñ','Ñ','ª','º','¿','⌐','¬','½','¼','¡','«','»',
    '░','▒','▓','│','┤','╡','╢','╖','╕','╣','║','╗','╝','╜','╛','┐',
    '└','┴','┬','├','─','┼','╞','╟','╚','╔','╩','╦','╠','═','╬','╧',
    '╨','╤','╥','╙','╘','╒','╓','╫','╪','┘','┌','█','▄','▌','▐','▀',
    'α','ß','Γ','π','Σ','σ','µ','τ','Φ','Θ','Ω','δ','∞','φ','ε','∩',
    '≡','±','≥','≤','⌠','⌡','÷','≈','°','∙','·','√','ⁿ','²','■','\u{a0}',
];

fn decode_name(raw: &[u8], utf8_flag: bool) -> String {
    if utf8_flag {
        return String::from_utf8_lossy(raw).into_owned();
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    raw.iter()
        .map(|&b| if b < 0x80 { b as char } else { CP437_HIGH[(b - 0x80) as usize] })
        .collect()
}

// ---------------- decompressing one entry ----------------

fn unsupported(e: &Entry) -> String {
    format!("{}: compression method {} ({}) is not supported",
        e.name, e.real_method(), e.method_name())
}

/// Read a decoder to the end, but never accept more than the entry declared.
///
/// Without this, a few hundred bytes of bzip2 can expand to gigabytes: the
/// declared size is only a hint and a hostile archive simply understates it.
/// Reading one byte past the declared size is enough to prove it lied.
fn read_bounded<R: Read>(mut r: R, e: &Entry, label: &str) -> Result<Vec<u8>, String> {
    let limit = e.usize_.saturating_add(1);
    let mut out = Vec::with_capacity(e.usize_.min(1 << 26) as usize);
    (&mut r)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|err| format!("{}: {} error: {}", e.name, label, err))?;
    if out.len() as u64 > e.usize_ {
        return Err(format!(
            "{}: entry expands beyond its declared size of {} - refusing to continue",
            e.name,
            fmt_size(e.usize_)
        ));
    }
    Ok(out)
}

/// Decompress fully into memory: used for small entries and encrypted ones.
fn inflate_all(raw: &[u8], e: &Entry, method: u16) -> Result<Vec<u8>, String> {
    let cap = e.usize_.min(1 << 30) as usize;
    match method {
        0 => Ok(raw.to_vec()),
        8 => {
            let opts = DeflateOptions::default()
                .set_size_hint(cap)
                .set_limit(e.usize_ as usize + 64);
            DeflateDecoder::new_with_options(raw, opts)
                .decode_deflate()
                .map_err(|err| format!("{}: deflate error: {:?}",
                                   e.name, err.error))
        }
        12 => read_bounded(bzip2::read::BzDecoder::new(raw), e, "bzip2"),
        93 => {
            let d = zstd::stream::read::Decoder::new(raw)
                .map_err(|err| format!("{}: zstd error: {}", e.name, err))?;
            read_bounded(d, e, "zstd")
        }
        95 => read_bounded(xz2::read::XzDecoder::new(raw), e, "xz"),
        _ => Err(unsupported(e)),
    }
}

/// What an entry produced: either a buffer, or bytes already sent to disk.
enum Produced {
    InMemory(Vec<u8>),
    Streamed { crc: u32, size: u64 },
}

/// Decrypt an entry, returning data that is still COMPRESSED.
fn decrypt(raw: &[u8], e: &Entry, password: Option<&str>) -> Result<Vec<u8>, String> {
    let pw = password.ok_or_else(|| {
        format!("{}: encrypted - use -p <password>", e.name)
    })?;

    if let Some(info) = &e.aes {
        return crypto::aes_decrypt(raw, info.strength, pw).map_err(|err| match err {
            CryptoError::WrongPassword => {
                format!("{}: wrong password", e.name)
            }
            CryptoError::Tampered => format!("{}: authentication failed - file was modified or corrupted", e.name),
            CryptoError::TooShort => {
                format!("{}: encrypted data too short", e.name)
            }
        });
    }

    if e.flags & (1 << 6) != 0 {
        return Err(format!("{}: PKWARE strong encryption is not supported", e.name));
    }
    if e.method == 99 {
        return Err(format!("{}: AES entry is missing its header (corrupt file)", e.name));
    }

    let check = if e.flags & (1 << 3) != 0 {
        (e.dos_time >> 8) as u8
    } else {
        (e.crc >> 24) as u8
    };
    crypto::zipcrypto_decrypt(raw, pw, check).map_err(|err| match err {
        CryptoError::WrongPassword => format!("{}: wrong password", e.name),
        _ => format!("{}: encrypted data too short", e.name),
    })
}

/// Stream-decompress into `w`, keeping memory flat. Returns (crc, bytes).
fn stream_to<W: Write>(
    raw: &[u8],
    e: &Entry,
    method: u16,
    w: &mut W,
    progress: &Progress,
) -> Result<(u32, u64), String> {
    let mut hasher = crc32fast::Hasher::new();
    let mut total = 0u64;

    let mut sink = |buf: &[u8], w: &mut W| -> Result<(), String> {
        hasher.update(buf);
        total += buf.len() as u64;
        w.write_all(buf)
            .map_err(|err| format!("{}: write error: {}", e.name, err))?;
        progress.add_bytes(buf.len() as u64);
        Ok(())
    };

    match method {
        0 => {
            for chunk in raw.chunks(IO_CHUNK) {
                sink(chunk, w)?;
            }
        }
        8 => {
            let mut d = flate2::Decompress::new(false);
            let mut out = vec![0u8; IO_CHUNK];
            let mut pos = 0usize;
            loop {
                let before_in = d.total_in();
                let before_out = d.total_out();
                let status = d
                    .decompress(&raw[pos..], &mut out, flate2::FlushDecompress::None)
                    .map_err(|err| {
                        format!("{}: deflate error: {}", e.name, err)
                    })?;
                let consumed = (d.total_in() - before_in) as usize;
                let produced = (d.total_out() - before_out) as usize;
                pos += consumed;
                if produced > 0 {
                    sink(&out[..produced], w)?;
                }
                match status {
                    flate2::Status::StreamEnd => break,
                    _ => {
                        if consumed == 0 && produced == 0 {
                            break;
                        }
                    }
                }
            }
        }
        12 => stream_reader(bzip2::read::BzDecoder::new(raw), e, w, &mut sink)?,
        93 => {
            let dec = zstd::stream::read::Decoder::new(raw)
                .map_err(|err| format!("{}: zstd error: {}", e.name, err))?;
            stream_reader(dec, e, w, &mut sink)?
        }
        95 => stream_reader(xz2::read::XzDecoder::new(raw), e, w, &mut sink)?,
        _ => return Err(unsupported(e)),
    }

    Ok((hasher.finalize(), total))
}

/// Pump a decoder into the sink, stopping the moment it produces more than the
/// entry declared. Streaming to disk turns an unbounded expansion into a full
/// volume rather than an out-of-memory abort, so the cap matters here too.
fn stream_reader<R: Read, W: Write, F>(
    r: R,
    e: &Entry,
    w: &mut W,
    sink: &mut F,
) -> Result<(), String>
where
    F: FnMut(&[u8], &mut W) -> Result<(), String>,
{
    let mut limited = r.take(e.usize_.saturating_add(1));
    let mut buf = vec![0u8; IO_CHUNK];
    let mut written = 0u64;
    loop {
        let n = limited
            .read(&mut buf)
            .map_err(|err| format!("{}: read error: {}", e.name, err))?;
        if n == 0 {
            return Ok(());
        }
        written += n as u64;
        if written > e.usize_ {
            return Err(format!(
                "{}: entry expands beyond its declared size of {} - refusing to continue",
                e.name,
                fmt_size(e.usize_)
            ));
        }
        sink(&buf[..n], w)?;
    }
}

// ---------------- extraction plan ----------------

pub struct Plan {
    pub files: Vec<(Entry, PathBuf)>,
    pub dirs: Vec<PathBuf>,
    pub skipped_unsafe: Vec<String>,
    pub filtered_out: usize,
}

pub fn build_plan(entries: Vec<Entry>, root: &std::path::Path, opts: &Options) -> Plan {
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    let mut files: Vec<(Entry, PathBuf)> = Vec::new();
    let mut skipped_unsafe = Vec::new();
    let mut filtered_out = 0usize;

    for e in entries {
        if !opts.filter.matches(&e.name) {
            filtered_out += 1;
            continue;
        }
        let rel = match safepath::sanitize(&e.name) {
            Ok(r) => r,
            Err(Reject::Traversal) => {
                skipped_unsafe.push(format!("{} (path escapes target folder)", e.name));
                continue;
            }
            Err(Reject::Empty) => {
                skipped_unsafe.push(format!("{} (empty name)", e.name));
                continue;
            }
        };
        // -e / --flat: drop folder structure, keep the file name only
        let rel = if opts.flatten {
            match rel.file_name() {
                Some(n) => PathBuf::from(n),
                None => continue,
            }
        } else {
            rel
        };
        let full = root.join(&rel);
        if e.is_dir {
            dirs.insert(full);
        } else {
            if let Some(parent) = full.parent() {
                dirs.insert(parent.to_path_buf());
            }
            files.push((e, full));
        }
    }

    // Duplicate names (Windows is case-insensitive): keep the LAST copy so
    // two threads never write the same file at once.
    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<(Entry, PathBuf)> = Vec::with_capacity(files.len());
    for item in files.into_iter().rev() {
        if seen.insert(item.1.to_string_lossy().to_lowercase()) {
            kept.push(item);
        }
    }
    kept.reverse();

    Plan { files: kept, dirs: dirs.into_iter().collect(), skipped_unsafe, filtered_out }
}

// ---------------- the 'l' command ----------------

pub fn run_list(opts: &Options, mmap: &Mmap) -> i32 {
    let entries = match parse(mmap) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };

    println!(
        "{:>14}  {:>14}  {:<9} {:<10} Name",
        "Size",
        "Packed",
        "Method",
        "Crypto"
    );
    println!("{:-<14}  {:-<14}  {:-<9} {:-<10} {:-<30}", "", "", "", "", "");

    let mut total_u = 0u64;
    let mut total_c = 0u64;
    let mut n = 0u64;
    for e in &entries {
        if e.is_dir || !opts.filter.matches(&e.name) {
            continue;
        }
        n += 1;
        total_u += e.usize_;
        total_c += e.csize;
        println!(
            "{:>14}  {:>14}  {:<9} {:<10} {}",
            e.usize_, e.csize, e.method_name(), e.enc_name(), e.name
        );
    }
    println!("{:-<14}  {:-<14}  {:-<9} {:-<10} {:-<30}", "", "", "", "", "");
    let ratio = if total_u > 0 { total_c as f64 / total_u as f64 * 100.0 } else { 0.0 };
    println!(
        "{:>14}  {:>14}  {} files, {:.1}% of original",
        fmt_size(total_u),
        fmt_size(total_c),
        n,
        ratio
    );
    common::EXIT_OK
}

// ---------------- the 'x' and 't' commands ----------------

pub fn run_extract(opts: &Options, mmap: &Mmap, mode: Mode) -> i32 {
    let t0 = Instant::now();
    let testing = mode == Mode::Test;

    let entries = match parse(mmap) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };

    // Ask for the password up front when any entry needs one
    let mut password = opts.password.clone();
    if password.is_none() && entries.iter().any(|e| !e.is_dir && e.encrypted()) {
        match crate::cli::ask_password() {
            Some(pw) => password = Some(pw),
            None => {
                eprintln!("fzip: archive is encrypted - use -p <password>");
                return common::EXIT_FATAL;
            }
        }
    }

    // Destination in extended form, so paths beyond 260 chars still work
    let root = if testing {
        PathBuf::new()
    } else {
        let want = opts.resolve_out_dir();
        match safepath::prepare_root(&want) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("fzip: cannot create output folder {}: {}",
                                          want.display(), e);
                return common::EXIT_FATAL;
            }
        }
    };

    let plan = build_plan(entries, &root, opts);

    if !testing {
        for d in &plan.dirs {
            let _ = fs::create_dir_all(d);
        }
    }

    let mut files = plan.files;
    // Largest first, so all threads finish at roughly the same time
    files.sort_by(|a, b| b.0.usize_.cmp(&a.0.usize_));

    let total_bytes: u64 = files.iter().map(|(e, _)| e.usize_).sum();

    // Warn on an implausible expansion ratio, the classic zip-bomb signature
    let archive_len = mmap.len() as u64;
    if archive_len > 0 && total_bytes / archive_len.max(1) > 500 && total_bytes > 1 << 30 {
        eprintln!(
            "fzip: warning: archive expands {}x to {} - possible zip bomb",
            total_bytes / archive_len,
            fmt_size(total_bytes)
        );
    }

    let show_bar = common::progress_enabled(opts.force_progress, opts.quiet, opts.verbose);
    let progress = Progress::new(total_bytes, files.len() as u64, show_bar);

    if !opts.quiet {
        println!(
            "{}",
            if testing {
                format!("Testing {} ({} files, {})...",
                    safepath::display(&opts.archive), files.len(), fmt_size(total_bytes))
            } else {
                format!("Extracting {} ({} files, {}) -> {}",
                    safepath::display(&opts.archive), files.len(), fmt_size(total_bytes),
                    safepath::display(&root))
            }
        );
    }
    let bar = progress.spawn(t0);

    let data: &[u8] = mmap;
    let pw = password.as_deref();
    let skipped_exist = AtomicU64::new(0);
    let prog = progress.clone();

    let errors: Vec<String> = files
        .par_iter()
        .with_max_len(1)
        .filter_map(|(e, path)| {
            extract_one(data, e, path, opts, pw, &prog, testing, &skipped_exist).err()
        })
        .collect();

    progress.stop();
    if let Some(h) = bar {
        let _ = h.join();
    }

    report(
        opts, t0, &progress, &errors, &plan.skipped_unsafe, plan.filtered_out,
        skipped_exist.load(Ordering::Relaxed), files.len(), archive_len, testing,
    )
}

#[allow(clippy::too_many_arguments)]
fn extract_one(
    data: &[u8],
    e: &Entry,
    path: &PathBuf,
    opts: &Options,
    pw: Option<&str>,
    progress: &Arc<Progress>,
    testing: bool,
    skipped_exist: &AtomicU64,
) -> Result<(), String> {
    // Overwrite policy
    let final_path = if testing {
        path.clone()
    } else {
        match resolve_overwrite(path, e, opts) {
            Some(p) => p,
            None => {
                skipped_exist.fetch_add(1, Ordering::Relaxed);
                progress.add_bytes(e.usize_);
                progress.add_file();
                return Ok(());
            }
        }
    };

    progress.set_current(&e.name);
    let start = data_start(data, e)?;
    let raw = &data[start..start + e.csize as usize];
    let method = e.real_method();

    // AE-2 stores no CRC; the field is always 0
    let skip_crc = matches!(&e.aes, Some(i) if i.vendor_version == 2);
    let want_crc = opts.check_crc && e.usize_ > 0 && !skip_crc;

    // Path 1: encrypted entries must be buffered, as HMAC covers the whole payload
    let produced = if e.encrypted() {
        if e.usize_ > opts.max_memory {
            return Err(format!("{}: encrypted entry is {} - exceeds memory limit {} (raise with --max-memory)",
                e.name, fmt_size(e.usize_), fmt_size(opts.max_memory)));
        }
        let compressed = decrypt(raw, e, pw)?;
        let out = if method == 0 { compressed } else { inflate_all(&compressed, e, method)? };
        Produced::InMemory(out)
    } else if e.usize_ >= STREAM_THRESHOLD {
        // Path 2: large entries stream to disk, so RAM stays flat
        if testing {
            let mut sink = std::io::sink();
            let (crc, size) = stream_to(raw, e, method, &mut sink, progress)?;
            Produced::Streamed { crc, size }
        } else {
            let f = create_file(&final_path, e)?;
            let mut w = std::io::BufWriter::with_capacity(IO_CHUNK, &f);
            let (crc, size) = stream_to(raw, e, method, &mut w, progress)?;
            w.flush().map_err(|err| format!("{}: write error: {}", e.name, err))?;
            drop(w);
            set_mtime(&f, e);
            Produced::Streamed { crc, size }
        }
    } else {
        // Path 3: small entries decompress in one shot, which is fastest
        let out = match method {
            0 => Cow::Borrowed(raw),
            m => Cow::Owned(inflate_all(raw, e, m)?),
        };
        Produced::InMemory(out.into_owned())
    };

    match produced {
        Produced::Streamed { crc, size } => {
            if want_crc && crc != e.crc {
                return Err(crc_error(e, crc));
            }
            if size != e.usize_ && e.usize_ > 0 {
                return Err(format!("{}: size mismatch (expected {}, got {})",
                               e.name, e.usize_, size));
            }
        }
        Produced::InMemory(out) => {
            if want_crc {
                let crc = crc32fast::hash(&out);
                if crc != e.crc {
                    return Err(crc_error(e, crc));
                }
            }
            if !testing {
                let f = create_file(&final_path, e)?;
                {
                    let mut w = &f;
                    for chunk in out.chunks(IO_CHUNK) {
                        w.write_all(chunk).map_err(|err| {
                            format!("{}: write error: {}", e.name, err)
                        })?;
                        progress.add_bytes(chunk.len() as u64);
                    }
                }
                set_mtime(&f, e);
            } else {
                progress.add_bytes(out.len() as u64);
            }
        }
    }

    progress.add_file();
    if opts.verbose {
        println!("  {}", e.name);
    }
    Ok(())
}

fn crc_error(e: &Entry, got: u32) -> String {
    format!("{}: CRC mismatch - file is corrupt (expected {:08x}, got {:08x})",
        e.name, e.crc, got)
}

fn create_file(path: &PathBuf, e: &Entry) -> Result<fs::File, String> {
    match fs::File::create(path) {
        Ok(f) => Ok(f),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            // Two ordinary situations land here, and both are recoverable.
            //
            // The target is a directory: an archive can name the same path as
            // both a folder and a file. Report it rather than destroying the
            // folder and everything under it.
            if path.is_dir() {
                return Err(format!(
                    "{}: a folder of that name already exists here",
                    e.name
                ));
            }
            // The target is read-only, which is what re-installing over a
            // previous version looks like. Clear the flag and retry once;
            // `--overwrite all` means overwrite.
            if let Ok(md) = fs::metadata(path) {
                let mut perms = md.permissions();
                if perms.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    if fs::set_permissions(path, perms).is_ok() {
                        if let Ok(f) = fs::File::create(path) {
                            return Ok(f);
                        }
                    }
                }
            }
            Err(format!(
                "{}: cannot create file: {} - the file may be open in another program",
                e.name, err
            ))
        }
        Err(err) => Err(format!("{}: cannot create file: {}", e.name, err)),
    }
}

fn set_mtime(f: &fs::File, e: &Entry) {
    if let Some(ts) = common::dos_to_unix(e.dos_date, e.dos_time) {
        let ft = filetime::FileTime::from_unix_time(ts, 0);
        let _ = filetime::set_file_handle_times(f, None, Some(ft));
    }
}

/// Where to write, or None when the entry should be skipped.
fn resolve_overwrite(path: &PathBuf, e: &Entry, opts: &Options) -> Option<PathBuf> {
    if !path.exists() {
        return Some(path.clone());
    }
    match opts.overwrite {
        Overwrite::Always => Some(path.clone()),
        Overwrite::Skip => None,
        Overwrite::Newer => {
            let entry_ts = common::dos_to_unix(e.dos_date, e.dos_time).unwrap_or(0);
            let disk_ts = fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if entry_ts > disk_ts { Some(path.clone()) } else { None }
        }
        Overwrite::Rename => {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ext = path.extension().map(|s| format!(".{}", s.to_string_lossy()))
                .unwrap_or_default();
            let parent = path.parent()?;
            // Claim the name atomically with create_new. A plain `exists()` test
            // would let two worker threads pick the same candidate and have one
            // silently overwrite the other.
            for i in 1..10_000 {
                let cand = parent.join(format!("{}_{}{}", stem, i, ext));
                match fs::OpenOptions::new().write(true).create_new(true).open(&cand) {
                    Ok(_) => return Some(cand),
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return None,
                }
            }
            None
        }
    }
}

// ---------------- final report ----------------

#[allow(clippy::too_many_arguments)]
fn report(
    opts: &Options,
    t0: Instant,
    progress: &Arc<Progress>,
    errors: &[String],
    skipped_unsafe: &[String],
    filtered_out: usize,
    skipped_exist: u64,
    total_planned: usize,
    archive_len: u64,
    testing: bool,
) -> i32 {
    for err in errors {
        eprintln!("fzip: ERROR: {}", err);
    }
    for s in skipped_unsafe {
        eprintln!("fzip: SKIPPED (unsafe): {}", s);
    }

    if opts.quiet {
        return if errors.is_empty() { common::EXIT_OK } else { common::EXIT_FATAL };
    }

    let secs = t0.elapsed().as_secs_f64();
    let done = progress.bytes_done.load(Ordering::Relaxed);
    let ok = total_planned - errors.len() - skipped_exist as usize;
    let speed = if secs > 0.0 { (done as f64 / secs) as u64 } else { 0 };

    if testing {
        if errors.is_empty() {
            println!("OK: {} files verified, {} in {:.3}s ({}/s)",
                               ok, fmt_size(done), secs, fmt_size(speed));
        } else {
            println!("FAILED: {} of {} files are damaged",
                               errors.len(), total_planned);
        }
    } else {
        println!("Done: {} files, {} -> {} in {:.3}s ({}/s)",
                           ok, fmt_size(archive_len), fmt_size(done), secs, fmt_size(speed));
    }

    if skipped_exist > 0 {
        println!("  {} files skipped (already exist)", skipped_exist);
    }
    if filtered_out > 0 {
        println!("  {} entries excluded by filters", filtered_out);
    }
    if !skipped_unsafe.is_empty() {
        println!("  {} entries skipped as unsafe", skipped_unsafe.len());
    }
    if !errors.is_empty() {
        println!("  {} errors (see above)", errors.len());
    }

    if !errors.is_empty() {
        common::EXIT_FATAL
    } else if !skipped_unsafe.is_empty() || skipped_exist > 0 {
        common::EXIT_WARNING
    } else {
        common::EXIT_OK
    }
}
