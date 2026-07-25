//! ZIP writing: parallel multi-threaded compression, ZIP64, AES-256.

use std::fs;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use libdeflater::{CompressionLvl, Compressor};
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::cli::Options;
use crate::common::{self, fmt_size, Progress};
use crate::crypto;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CDIR: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_EOCD64: u32 = 0x0606_4b50;
const SIG_EOCD64_LOC: u32 = 0x0706_4b50;

/// 0xFFFFFFFF is the ZIP64 *sentinel*, not a valid stored value: a field whose
/// real value equals it must be moved into a ZIP64 record. Hence `>=`, never `>`.
const ZIP64_LIMIT: u64 = 0xFFFF_FFFF;
/// Same reasoning for the 16-bit entry count in the end-of-archive record.
const ZIP64_COUNT_LIMIT: u64 = 0xFFFF;
/// Files above this size are compressed as a stream, never buffered whole.
const STREAM_THRESHOLD: u64 = 32 * 1024 * 1024;
const IO_CHUNK: usize = 1024 * 1024;

/// One file destined for the archive.
struct Source {
    disk: PathBuf,
    name: String,
    size: u64,
    mtime: i64,
    is_dir: bool,
}

/// A compressed entry, ready to be written out.
struct Packed {
    name: String,
    method: u16,
    crc: u32,
    usize_: u64,
    data: Vec<u8>,
    dos_date: u16,
    dos_time: u16,
    is_dir: bool,
}

/// Record used to build the central directory.
struct CdEntry {
    name: String,
    method: u16,
    crc: u32,
    csize: u64,
    usize_: u64,
    offset: u64,
    dos_date: u16,
    dos_time: u16,
    is_dir: bool,
    encrypted: bool,
    /// Set when the local header already committed to ZIP64 sentinels. The
    /// central record must then agree, even if the final sizes turned out small.
    force_zip64: bool,
}

// ---------------- collecting source files ----------------

fn mtime_of(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Normalise the stored name: forward slashes, no drive letter, no `..`.
fn archive_name(rel: &Path, is_dir: bool) -> String {
    let mut s = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if is_dir && !s.is_empty() && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// The ZIP name-length field is 16 bits, so a longer name cannot be recorded.
const MAX_NAME_LEN: usize = 0xFFFF;

fn collect(inputs: &[PathBuf], opts: &Options) -> Result<Vec<Source>, String> {
    let mut out = Vec::new();
    // Never swallow the archive we are currently writing. It exists on disk the
    // moment we create it, and reading a file that is concurrently growing
    // produces a garbage entry of nondeterministic size.
    let self_path = fs::canonicalize(&opts.archive).ok();
    let is_self = |p: &Path| -> bool {
        match (&self_path, fs::canonicalize(p).ok()) {
            (Some(a), Some(b)) => *a == b,
            _ => false,
        }
    };

    for input in inputs {
        let md = fs::metadata(input)
            .map_err(|e| format!("cannot read {}: {}", input.display(), e))?;

        if md.is_file() {
            if is_self(input) {
                continue;
            }
            let name = input
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            if opts.filter.matches(&name) {
                out.push(Source {
                    disk: input.clone(),
                    name,
                    size: md.len(),
                    mtime: mtime_of(&md),
                    is_dir: false,
                });
            }
            continue;
        }

        // Directory: keep the top folder name as the stored prefix
        let base = input.parent().unwrap_or(Path::new(""));
        for entry in WalkDir::new(input).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("fzip: warning: skipping {}", e);
                    continue;
                }
            };
            let rel = entry.path().strip_prefix(base).unwrap_or(entry.path());
            let is_dir = entry.file_type().is_dir();
            let name = archive_name(rel, is_dir);
            if name.is_empty() || !opts.filter.matches(&name) {
                continue;
            }
            if name.len() > MAX_NAME_LEN {
                eprintln!(
                    "fzip: warning: skipping {} - name is {} bytes, over the ZIP limit of {}",
                    entry.path().display(),
                    name.len(),
                    MAX_NAME_LEN
                );
                continue;
            }
            if !is_dir && is_self(entry.path()) {
                continue;
            }
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            out.push(Source {
                disk: entry.path().to_path_buf(),
                name,
                size: if is_dir { 0 } else { md.len() },
                mtime: mtime_of(&md),
                is_dir,
            });
        }
    }
    Ok(out)
}

// ---------------- compression ----------------

fn level_of(opts: &Options) -> i32 {
    // 0 = store only; 1..9 maps onto libdeflate's 1..12
    match opts.level {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        4 => 5,
        5 => 6,
        6 => 7,
        7 => 9,
        8 => 11,
        _ => 12,
    }
}

fn deflate_block(data: &[u8], lvl: i32) -> Vec<u8> {
    let mut c = Compressor::new(CompressionLvl::new(lvl).unwrap_or_default());
    let mut out = vec![0u8; c.deflate_compress_bound(data.len())];
    match c.deflate_compress(data, &mut out) {
        Ok(n) => {
            out.truncate(n);
            out
        }
        Err(_) => Vec::new(),
    }
}

fn pack_one(src: &Source, opts: &Options, lvl: i32) -> Result<Packed, String> {
    let (dos_date, dos_time) = common::unix_to_dos(src.mtime);

    if src.is_dir {
        return Ok(Packed {
            name: src.name.clone(),
            method: 0,
            crc: 0,
            usize_: 0,
            data: Vec::new(),
            dos_date,
            dos_time,
            is_dir: true,
        });
    }

    let mut raw = Vec::with_capacity(src.size as usize);
    fs::File::open(&src.disk)
        .and_then(|mut f| f.read_to_end(&mut raw))
        .map_err(|e| format!("cannot read {}: {}", src.disk.display(), e))?;

    let crc = crc32fast::hash(&raw);
    let usize_ = raw.len() as u64;

    let (method, mut data) = if lvl == 0 {
        (0u16, raw)
    } else {
        let comp = deflate_block(&raw, lvl);
        // Store verbatim when compression does not help, as 7-Zip/WinRAR do
        if comp.is_empty() || comp.len() >= raw.len() {
            (0u16, raw)
        } else {
            (8u16, comp)
        }
    };

    if let Some(pw) = &opts.password {
        data = crypto::aes_encrypt(&data, pw)?;
    }

    Ok(Packed { name: src.name.clone(), method, crc, usize_, data, dos_date, dos_time, is_dir: false })
}

// ---------------- writing the file ----------------

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// ZIP64 extra field for a local header.
fn zip64_local_extra(usize_: u64, csize: u64) -> Vec<u8> {
    let mut e = Vec::new();
    put_u16(&mut e, 0x0001);
    put_u16(&mut e, 16);
    put_u64(&mut e, usize_);
    put_u64(&mut e, csize);
    e
}

/// "Version needed to extract", in APPNOTE's tenths-of-a-version encoding.
/// WinZip AES requires 5.1; ZIP64 requires 4.5; plain deflate 2.0.
fn version_needed(encrypted: bool, need64: bool) -> u16 {
    if encrypted {
        51
    } else if need64 {
        45
    } else {
        20
    }
}

/// AES extra field (WinZip AE-2, AES-256).
fn aes_extra() -> Vec<u8> {
    let mut e = Vec::new();
    put_u16(&mut e, 0x9901);
    put_u16(&mut e, 7);
    put_u16(&mut e, 2); // AE-2
    e.extend_from_slice(b"AE");
    e.push(3); // AES-256
    put_u16(&mut e, 8); // phuong thuc nen ben trong (se ghi de neu store)
    e
}

fn write_local<W: Write>(
    w: &mut W,
    p: &Packed,
    encrypted: bool,
    need64: bool,
) -> Result<u64, String> {
    let name = p.name.as_bytes();
    let csize = p.data.len() as u64;

    let mut extra = Vec::new();
    if need64 {
        extra.extend_from_slice(&zip64_local_extra(p.usize_, csize));
    }
    if encrypted {
        let mut ae = aes_extra();
        // patch in the real compression method
        let n = ae.len();
        ae[n - 2] = (p.method & 0xFF) as u8;
        ae[n - 1] = (p.method >> 8) as u8;
        extra.extend_from_slice(&ae);
    }

    let mut hdr = Vec::with_capacity(30 + name.len() + extra.len());
    put_u32(&mut hdr, SIG_LOCAL);
    put_u16(&mut hdr, version_needed(encrypted, need64));
    put_u16(&mut hdr, 1 << 11 | if encrypted { 1 } else { 0 }); // UTF-8 + co ma hoa
    put_u16(&mut hdr, if encrypted { 99 } else { p.method });
    put_u16(&mut hdr, p.dos_time);
    put_u16(&mut hdr, p.dos_date);
    put_u32(&mut hdr, if encrypted { 0 } else { p.crc }); // AE-2: CRC = 0
    put_u32(&mut hdr, if need64 { ZIP64_LIMIT as u32 } else { csize as u32 });
    put_u32(&mut hdr, if need64 { ZIP64_LIMIT as u32 } else { p.usize_ as u32 });
    put_u16(&mut hdr, name.len() as u16);
    put_u16(&mut hdr, extra.len() as u16);
    hdr.extend_from_slice(name);
    hdr.extend_from_slice(&extra);

    w.write_all(&hdr)
        .map_err(|e| format!("write error: {}", e))?;
    w.write_all(&p.data)
        .map_err(|e| format!("write error: {}", e))?;
    Ok(hdr.len() as u64 + csize)
}

fn write_central<W: Write>(w: &mut W, entries: &[CdEntry]) -> Result<u64, String> {
    let mut total = 0u64;
    for e in entries {
        let name = e.name.as_bytes();
        let need64 = e.force_zip64
            || e.usize_ >= ZIP64_LIMIT
            || e.csize >= ZIP64_LIMIT
            || e.offset >= ZIP64_LIMIT;

        let mut extra = Vec::new();
        if need64 {
            let mut z = Vec::new();
            put_u64(&mut z, e.usize_);
            put_u64(&mut z, e.csize);
            put_u64(&mut z, e.offset);
            put_u16(&mut extra, 0x0001);
            put_u16(&mut extra, z.len() as u16);
            extra.extend_from_slice(&z);
        }
        if e.encrypted {
            let mut ae = aes_extra();
            let n = ae.len();
            ae[n - 2] = (e.method & 0xFF) as u8;
            ae[n - 1] = (e.method >> 8) as u8;
            extra.extend_from_slice(&ae);
        }

        let mut hdr = Vec::with_capacity(46 + name.len() + extra.len());
        put_u32(&mut hdr, SIG_CDIR);
        put_u16(&mut hdr, 0x031E); // made by: Unix, ZIP 3.0
        put_u16(&mut hdr, version_needed(e.encrypted, need64));
        put_u16(&mut hdr, 1 << 11 | if e.encrypted { 1 } else { 0 });
        put_u16(&mut hdr, if e.encrypted { 99 } else { e.method });
        put_u16(&mut hdr, e.dos_time);
        put_u16(&mut hdr, e.dos_date);
        put_u32(&mut hdr, if e.encrypted { 0 } else { e.crc });
        put_u32(&mut hdr, if need64 { ZIP64_LIMIT as u32 } else { e.csize as u32 });
        put_u32(&mut hdr, if need64 { ZIP64_LIMIT as u32 } else { e.usize_ as u32 });
        put_u16(&mut hdr, name.len() as u16);
        put_u16(&mut hdr, extra.len() as u16);
        put_u16(&mut hdr, 0); // do dai ghi chu
        put_u16(&mut hdr, 0); // so dia
        put_u16(&mut hdr, 0); // thuoc tinh trong
        // external attributes: bit 0x10 = directory, Unix mode in high word
        // High 16 bits carry the Unix mode. 0x41ED = 040755 (drwxr-xr-x) and
        // 0x81A4 = 100644 (rw-r--r--); the low bit 0x10 is the DOS directory
        // flag. The previous directory value had no permission bits at all, so
        // extracted folders came out unreadable on Unix.
        let ext_attr = if e.is_dir { 0x41ED_0010u32 } else { 0x81A4_0000u32 };
        put_u32(&mut hdr, ext_attr);
        put_u32(&mut hdr, if need64 { ZIP64_LIMIT as u32 } else { e.offset as u32 });
        hdr.extend_from_slice(name);
        hdr.extend_from_slice(&extra);

        w.write_all(&hdr).map_err(|er| format!("write error: {}", er))?;
        total += hdr.len() as u64;
    }
    Ok(total)
}

fn write_end<W: Write>(
    w: &mut W,
    count: u64,
    cd_offset: u64,
    cd_size: u64,
) -> Result<(), String> {
    let need64 =
        count >= ZIP64_COUNT_LIMIT || cd_offset >= ZIP64_LIMIT || cd_size >= ZIP64_LIMIT;
    let mut buf = Vec::new();

    if need64 {
        let e64_at = cd_offset + cd_size;
        put_u32(&mut buf, SIG_EOCD64);
        put_u64(&mut buf, 44); // kich thuoc ban ghi con lai
        put_u16(&mut buf, 0x031E);
        put_u16(&mut buf, 45);
        put_u32(&mut buf, 0);
        put_u32(&mut buf, 0);
        put_u64(&mut buf, count);
        put_u64(&mut buf, count);
        put_u64(&mut buf, cd_size);
        put_u64(&mut buf, cd_offset);

        put_u32(&mut buf, SIG_EOCD64_LOC);
        put_u32(&mut buf, 0);
        put_u64(&mut buf, e64_at);
        put_u32(&mut buf, 1);
    }

    put_u32(&mut buf, SIG_EOCD);
    put_u16(&mut buf, 0);
    put_u16(&mut buf, 0);
    put_u16(&mut buf, count.min(0xFFFF) as u16);
    put_u16(&mut buf, count.min(0xFFFF) as u16);
    put_u32(&mut buf, cd_size.min(ZIP64_LIMIT) as u32);
    put_u32(&mut buf, cd_offset.min(ZIP64_LIMIT) as u32);
    put_u16(&mut buf, 0);

    w.write_all(&buf).map_err(|e| format!("write error: {}", e))
}

// ---------------- the 'a' command ----------------

pub fn run_add(opts: &Options) -> i32 {
    let t0 = Instant::now();

    if opts.inputs.is_empty() {
        eprintln!("fzip: no input files given");
        return common::EXIT_USAGE;
    }
    if opts.archive.exists() && !opts.assume_yes {
        eprintln!(
            "fzip: {} already exists - use -y to overwrite",
            opts.archive.display()
        );
        return common::EXIT_FATAL;
    }

    let sources = match collect(&opts.inputs, opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };
    if sources.is_empty() {
        eprintln!("fzip: nothing to add");
        return common::EXIT_FATAL;
    }

    let total_bytes: u64 = sources.iter().map(|s| s.size).sum();
    let n_files = sources.iter().filter(|s| !s.is_dir).count();

    let out = match fs::File::create(&opts.archive) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("fzip: cannot create {}: {}",
                                      opts.archive.display(), e);
            return common::EXIT_FATAL;
        }
    };
    let mut w = BufWriter::with_capacity(4 * IO_CHUNK, out);
    let mut offset = 0u64;
    let mut cd: Vec<CdEntry> = Vec::with_capacity(sources.len());
    let mut errors: Vec<String> = Vec::new();

    let show_bar = common::progress_enabled(opts.force_progress, opts.quiet, opts.verbose);
    let progress = Progress::new(total_bytes, n_files as u64, show_bar);

    if !opts.quiet {
        println!(
            "Creating {} from {} files ({})...",
            opts.archive.display(),
            n_files,
            fmt_size(total_bytes)
        );
    }
    let bar = progress.spawn(t0);
    let lvl = level_of(opts);

    // Batch to bound RAM: compress a batch in parallel, then write in order.
    //
    // A buffered file does not cost its own size in RAM, it costs a multiple:
    // the raw bytes, the compression output buffer, and for encryption two more
    // copies. Budgeting on the raw size alone would overshoot `--max-memory`
    // several times over, so every accounting decision below uses the multiple.
    let budget = opts.max_memory.max(64 << 20);
    let cost_factor: u64 = if opts.password.is_some() { 4 } else { 2 };
    // Above this size a file is streamed instead of buffered.
    let stream_above = (budget / 2 / cost_factor).max(STREAM_THRESHOLD);
    let mut batch: Vec<&Source> = Vec::new();
    let mut batch_bytes = 0u64;

    // A failed write leaves an unknown number of bytes in the file, so `offset`
    // can no longer be trusted. Continuing would make a later streamed entry
    // seek backwards into the previous entry's payload and corrupt it silently.
    // Any write failure therefore aborts the whole archive.
    let flush_batch = |batch: &mut Vec<&Source>,
                           w: &mut BufWriter<fs::File>,
                           offset: &mut u64,
                           cd: &mut Vec<CdEntry>,
                           errors: &mut Vec<String>,
                           fatal: &mut bool| {
        if batch.is_empty() {
            return;
        }
        let packed: Vec<Result<Packed, String>> =
            batch.par_iter().map(|s| pack_one(s, opts, lvl)).collect();
        for r in packed {
            match r {
                Ok(p) => {
                    let csize = p.data.len() as u64;
                    let need64 =
                        p.usize_ >= ZIP64_LIMIT || csize >= ZIP64_LIMIT || *offset >= ZIP64_LIMIT;
                    let encrypted = opts.password.is_some() && !p.is_dir;
                    match write_local(w, &p, encrypted, need64) {
                        Ok(written) => {
                            cd.push(CdEntry {
                                name: p.name,
                                method: p.method,
                                crc: p.crc,
                                csize,
                                usize_: p.usize_,
                                offset: *offset,
                                dos_date: p.dos_date,
                                dos_time: p.dos_time,
                                is_dir: p.is_dir,
                                encrypted,
                                force_zip64: need64,
                            });
                            *offset += written;
                            progress.add_bytes(p.usize_);
                            if !p.is_dir {
                                progress.add_file();
                            }
                        }
                        Err(e) => {
                            errors.push(e);
                            *fatal = true;
                            break;
                        }
                    }
                }
                // A source that could not be read is reported and skipped; the
                // archive itself is still consistent.
                Err(e) => errors.push(e),
            }
        }
        batch.clear();
    };

    let mut fatal = false;
    for s in &sources {
        // Very large file: stream it separately so RAM stays flat
        // Large files stream through compression and encryption alike, so RAM
        // stays flat no matter how big the input or whether -p was given.
        if s.size >= stream_above && !s.is_dir {
            flush_batch(&mut batch, &mut w, &mut offset, &mut cd, &mut errors, &mut fatal);
            if fatal {
                break;
            }
            batch_bytes = 0;
            match stream_add(&mut w, s, opts, offset, &progress) {
                Ok((entry, written)) => {
                    offset += written;
                    cd.push(entry);
                    progress.add_file();
                }
                Err(e) => {
                    errors.push(e);
                    fatal = true;
                    break;
                }
            }
            continue;
        }

        batch.push(s);
        batch_bytes += s.size * cost_factor;
        if batch_bytes >= budget {
            flush_batch(&mut batch, &mut w, &mut offset, &mut cd, &mut errors, &mut fatal);
            if fatal {
                break;
            }
            batch_bytes = 0;
        }
    }
    if !fatal {
        flush_batch(&mut batch, &mut w, &mut offset, &mut cd, &mut errors, &mut fatal);
    }

    if fatal {
        drop(w);
        progress.stop();
        if let Some(h) = bar {
            let _ = h.join();
        }
        for e in &errors {
            eprintln!("fzip: ERROR: {}", e);
        }
        // Half an archive is worse than none: a reader would accept it silently.
        let _ = fs::remove_file(&opts.archive);
        eprintln!(
            "fzip: writing failed - removed the incomplete archive {}",
            opts.archive.display()
        );
        return common::EXIT_FATAL;
    }

    let cd_offset = offset;
    let cd_size = match write_central(&mut w, &cd) {
        Ok(n) => n,
        Err(e) => {
            errors.push(e);
            0
        }
    };
    if let Err(e) = write_end(&mut w, cd.len() as u64, cd_offset, cd_size) {
        errors.push(e);
    }
    if let Err(e) = w.flush() {
        errors.push(format!("write error: {}", e));
    }
    drop(w);

    progress.stop();
    if let Some(h) = bar {
        let _ = h.join();
    }

    for e in &errors {
        eprintln!("fzip: ERROR: {}", e);
    }

    if !opts.quiet {
        let secs = t0.elapsed().as_secs_f64();
        let archive_size = fs::metadata(&opts.archive).map(|m| m.len()).unwrap_or(0);
        let ratio = if total_bytes > 0 {
            archive_size as f64 / total_bytes as f64 * 100.0
        } else {
            0.0
        };
        let speed = if secs > 0.0 { (total_bytes as f64 / secs) as u64 } else { 0 };
        println!(
            "Done: {} files, {} -> {} ({:.1}%) in {:.3}s ({}/s)",
            n_files,
            fmt_size(total_bytes),
            fmt_size(archive_size),
            ratio,
            secs,
            fmt_size(speed)
        );
        if opts.password.is_some() {
            println!("  encrypted with AES-256");
        }
    }

    if errors.is_empty() { common::EXIT_OK } else { common::EXIT_FATAL }
}

/// Compress one very large file as a stream, writing straight into the archive.
fn stream_add(
    w: &mut BufWriter<fs::File>,
    s: &Source,
    opts: &Options,
    offset: u64,
    progress: &Progress,
) -> Result<(CdEntry, u64), String> {
    let (dos_date, dos_time) = common::unix_to_dos(s.mtime);
    let lvl = opts.level.clamp(0, 9) as u32;
    let method: u16 = if lvl == 0 { 0 } else { 8 };
    let encrypted = opts.password.is_some();

    // Write a placeholder local header; sizes are patched in afterwards
    let name = s.name.as_bytes();
    let mut extra = zip64_local_extra(0, 0);
    if encrypted {
        let mut ae = aes_extra();
        let n = ae.len();
        ae[n - 2] = (method & 0xFF) as u8;
        ae[n - 1] = (method >> 8) as u8;
        extra.extend_from_slice(&ae);
    }

    let mut hdr = Vec::new();
    put_u32(&mut hdr, SIG_LOCAL);
    put_u16(&mut hdr, version_needed(encrypted, true));
    put_u16(&mut hdr, 1 << 11 | if encrypted { 1 } else { 0 });
    put_u16(&mut hdr, if encrypted { 99 } else { method });
    put_u16(&mut hdr, dos_time);
    put_u16(&mut hdr, dos_date);
    put_u32(&mut hdr, 0); // CRC: patched later, and stays 0 for AE-2
    put_u32(&mut hdr, ZIP64_LIMIT as u32);
    put_u32(&mut hdr, ZIP64_LIMIT as u32);
    put_u16(&mut hdr, name.len() as u16);
    put_u16(&mut hdr, extra.len() as u16);
    hdr.extend_from_slice(name);
    // The ZIP64 field is written first, so this offset still points at it.
    let extra_at = offset + hdr.len() as u64;
    let crc_at = offset + 14;
    hdr.extend_from_slice(&extra);
    w.write_all(&hdr).map_err(|e| format!("write error: {}", e))?;

    let mut f = fs::File::open(&s.disk)
        .map_err(|e| format!("cannot read {}: {}", s.disk.display(), e))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut usize_ = 0u64;
    let mut buf = vec![0u8; IO_CHUNK];

    // Read -> [deflate] -> [AES] -> file, one chunk at a time, so peak memory
    // stays at one buffer regardless of how large the file is.
    let mut sink = Payload::new(w, opts.password.as_deref(), lvl, method)?;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read error: {}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        usize_ += n as u64;
        sink.write_all(&buf[..n]).map_err(|e| format!("write error: {}", e))?;
        progress.add_bytes(n as u64);
    }
    let csize = sink.finish()?;

    let crc = hasher.finalize();

    // Seek back and fill in the real sizes and CRC
    w.flush().map_err(|e| format!("write error: {}", e))?;
    let file = w.get_mut();
    let end = file.stream_position().map_err(|e| format!("seek error: {}", e))?;
    if !encrypted {
        // AE-2 defines the CRC field as zero; only patch it for plain entries.
        file.seek(std::io::SeekFrom::Start(crc_at))
            .and_then(|_| file.write_all(&crc.to_le_bytes()))
            .map_err(|e| format!("seek error: {}", e))?;
    }
    let mut z = Vec::new();
    put_u16(&mut z, 0x0001);
    put_u16(&mut z, 16);
    put_u64(&mut z, usize_);
    put_u64(&mut z, csize);
    file.seek(std::io::SeekFrom::Start(extra_at))
        .and_then(|_| file.write_all(&z))
        .map_err(|e| format!("seek error: {}", e))?;
    file.seek(std::io::SeekFrom::Start(end))
        .map_err(|e| format!("seek error: {}", e))?;

    Ok((
        CdEntry {
            name: s.name.clone(),
            method,
            crc,
            csize,
            usize_,
            offset,
            dos_date,
            dos_time,
            // The streamed local header is always written in ZIP64 form, so the
            // central directory entry must match it regardless of final size.
            force_zip64: true,
            is_dir: false,
            encrypted,
        },
        end - offset,
    ))
}

/// The compress-then-encrypt chain under a streamed entry. Both stages are
/// optional, so this models all four combinations without duplicating the loop.
enum Payload<'a, 'b> {
    Raw { w: CountingWriter<'a, 'b> },
    Deflate { enc: flate2::write::DeflateEncoder<CountingWriter<'a, 'b>> },
    RawAes { aes: crypto::AesWriter<CountingWriter<'a, 'b>> },
    DeflateAes { enc: flate2::write::DeflateEncoder<crypto::AesWriter<CountingWriter<'a, 'b>>> },
}

impl<'a, 'b> Payload<'a, 'b> {
    fn new(
        w: &'a mut BufWriter<fs::File>,
        password: Option<&'b str>,
        lvl: u32,
        method: u16,
    ) -> Result<Self, String> {
        let cw = CountingWriter { inner: w, count: 0, _pw: std::marker::PhantomData };
        Ok(match (password, method) {
            (None, 0) => Payload::Raw { w: cw },
            (None, _) => Payload::Deflate {
                enc: flate2::write::DeflateEncoder::new(cw, flate2::Compression::new(lvl)),
            },
            (Some(p), 0) => Payload::RawAes { aes: crypto::AesWriter::new(cw, p)? },
            (Some(p), _) => Payload::DeflateAes {
                enc: flate2::write::DeflateEncoder::new(
                    crypto::AesWriter::new(cw, p)?,
                    flate2::Compression::new(lvl),
                ),
            },
        })
    }

    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Payload::Raw { w } => w.write_all(data),
            Payload::Deflate { enc } => enc.write_all(data),
            Payload::RawAes { aes } => aes.write_all(data),
            Payload::DeflateAes { enc } => enc.write_all(data),
        }
    }

    /// Flush every stage and report the number of bytes that reached the file.
    fn finish(self) -> Result<u64, String> {
        let wr = |e: std::io::Error| format!("write error: {}", e);
        Ok(match self {
            Payload::Raw { w } => w.count,
            Payload::Deflate { enc } => enc.finish().map_err(wr)?.count,
            Payload::RawAes { aes } => aes.finish()?.0.count,
            Payload::DeflateAes { enc } => enc.finish().map_err(wr)?.finish()?.0.count,
        })
    }
}

/// Counts bytes written, so streamed output size is known.
struct CountingWriter<'a, 'b> {
    inner: &'a mut BufWriter<fs::File>,
    count: u64,
    _pw: std::marker::PhantomData<&'b ()>,
}

impl Write for CountingWriter<'_, '_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
