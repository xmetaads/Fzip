//! Formats other than ZIP and RAR: 7z, tar, gzip, bzip2, xz, zstd.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;

use crate::cli::{Mode, Options};
use crate::common::{self, fmt_size, Progress};
use crate::safepath::{self, Reject};

const IO_CHUNK: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    SevenZ,
    Tar,
    Gz,
    Bz2,
    Xz,
    Zst,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::SevenZ => "7z",
            Kind::Tar => "tar",
            Kind::Gz => "gzip",
            Kind::Bz2 => "bzip2",
            Kind::Xz => "xz",
            Kind::Zst => "zstd",
        }
    }

    /// Suffixes to strip for a single compressed file, e.g. data.txt.gz -> data.txt
    fn strip_ext(self) -> &'static [&'static str] {
        match self {
            Kind::Gz => &["gz", "tgz"],
            Kind::Bz2 => &["bz2", "tbz", "tbz2"],
            Kind::Xz => &["xz", "txz"],
            Kind::Zst => &["zst", "tzst"],
            _ => &[],
        }
    }
}

/// Identify by magic bytes; the file extension is not trusted.
pub fn detect(sig: &[u8], full: &[u8]) -> Option<Kind> {
    if sig.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return Some(Kind::SevenZ);
    }
    if sig.starts_with(&[0x1F, 0x8B]) {
        return Some(Kind::Gz);
    }
    if sig.starts_with(b"BZh") {
        return Some(Kind::Bz2);
    }
    if sig.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        return Some(Kind::Xz);
    }
    if sig.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return Some(Kind::Zst);
    }
    if full.len() > 262 && &full[257..262] == b"ustar" {
        return Some(Kind::Tar);
    }
    None
}

/// Open a decompressing reader for the given format.
fn open_stream(path: &Path, kind: Kind) -> Result<Box<dyn Read>, String> {
    let f = fs::File::open(path)
        .map_err(|e| format!("cannot open {}: {}", path.display(), e))?;
    let r = std::io::BufReader::with_capacity(IO_CHUNK, f);
    Ok(match kind {
        Kind::Tar => Box::new(r),
        Kind::Gz => Box::new(flate2::read::MultiGzDecoder::new(r)),
        Kind::Bz2 => Box::new(bzip2::read::MultiBzDecoder::new(r)),
        Kind::Xz => Box::new(xz2::read::XzDecoder::new_multi_decoder(r)),
        Kind::Zst => Box::new(
            zstd::stream::read::Decoder::new(r)
                .map_err(|e| format!("zstd error: {}", e))?,
        ),
        Kind::SevenZ => return Err("7z".into()),
    })
}

/// Peek the first 512 bytes to spot a tar inside, then rejoin the stream.
fn peek_tar(mut r: Box<dyn Read>) -> Result<(bool, Box<dyn Read>), String> {
    let mut head = vec![0u8; 512];
    let mut n = 0usize;
    while n < head.len() {
        match r.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) => return Err(format!("read error: {}", e)),
        }
    }
    head.truncate(n);
    let is_tar = n >= 262 && &head[257..262] == b"ustar";
    Ok((is_tar, Box::new(std::io::Cursor::new(head).chain(r))))
}

/// Output name when the payload is a single file rather than a tar.
fn single_name(archive: &Path, kind: Kind) -> String {
    let base = archive
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    for ext in kind.strip_ext() {
        let suffix = format!(".{}", ext);
        if base.to_lowercase().ends_with(&suffix) {
            let stem = &base[..base.len() - suffix.len()];
            // .tgz -> .tar
            return match *ext {
                "tgz" | "tbz" | "tbz2" | "txz" | "tzst" => format!("{}.tar", stem),
                _ => stem.to_string(),
            };
        }
    }
    format!("{}.out", base)
}

// ---------------- dispatch ----------------

pub fn run(opts: &Options, kind: Kind, mode: Mode) -> i32 {
    match kind {
        Kind::SevenZ => run_7z(opts, mode),
        _ => run_tarlike(opts, kind, mode),
    }
}

// ---------------- tar and its compressed variants ----------------

fn run_tarlike(opts: &Options, kind: Kind, mode: Mode) -> i32 {
    let t0 = Instant::now();
    let listing = mode == Mode::List;
    let testing = mode == Mode::Test;

    let stream = match open_stream(&opts.archive, kind) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };
    let (is_tar, stream) = match peek_tar(stream) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };

    if !is_tar && kind != Kind::Tar {
        return single_file(opts, kind, stream, mode, t0);
    }
    if !is_tar {
        eprintln!("fzip: not a valid tar archive");
        return common::EXIT_FATAL;
    }

    let root = if listing || testing {
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

    let show_bar = !listing && common::progress_enabled(opts.force_progress, opts.quiet, opts.verbose);
    let progress = Progress::new(0, 0, show_bar);
    if !opts.quiet && !listing {
        println!("Reading {} archive {}...",
                           kind.label(), opts.archive.display());
    }
    let bar = progress.spawn(t0);

    let mut archive = tar::Archive::new(stream);
    let mut errors: Vec<String> = Vec::new();
    let mut skipped_unsafe: Vec<String> = Vec::new();
    let mut count = 0u64;
    let mut total = 0u64;
    let mut filtered = 0usize;

    if listing {
        println!("{:>14}  {:<10} Name", "Size", "Type");
        println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
    }

    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("fzip: cannot read tar: {}", e);
            progress.stop();
            return common::EXIT_FATAL;
        }
    };

    for item in entries {
        let mut entry = match item {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("tar read error: {}", e));
                break;
            }
        };
        let path = match entry.path() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => continue,
        };
        if !opts.filter.matches(&path) {
            filtered += 1;
            continue;
        }
        let size = entry.header().size().unwrap_or(0);
        let is_dir = entry.header().entry_type().is_dir();

        if listing {
            let kindstr = if is_dir {
                "dir"
            } else if entry.header().entry_type().is_symlink() {
                "link"
            } else {
                "file"
            };
            println!("{:>14}  {:<10} {}", size, kindstr, path);
            if !is_dir {
                count += 1;
                total += size;
            }
            continue;
        }

        // Skip links: on Windows they can be abused to escape the target folder
        if entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link() {
            skipped_unsafe.push(format!("{} (link not extracted)", path));
            continue;
        }

        let rel = match safepath::sanitize(&path) {
            Ok(r) => r,
            Err(Reject::Traversal) => {
                skipped_unsafe.push(format!("{} (path escapes target folder)", path));
                continue;
            }
            Err(Reject::Empty) => continue,
        };
        let rel = if opts.flatten {
            match rel.file_name() {
                Some(n) => PathBuf::from(n),
                None => continue,
            }
        } else {
            rel
        };

        if is_dir {
            if !testing {
                let _ = fs::create_dir_all(root.join(&rel));
            }
            continue;
        }

        progress.set_current(&path);
        let dest = root.join(&rel);
        if !testing {
            if let Some(p) = dest.parent() {
                let _ = fs::create_dir_all(p);
            }
        }

        let written = if testing {
            copy_stream(&mut entry, &mut std::io::sink(), &progress)
        } else {
            match fs::File::create(&dest) {
                Ok(f) => {
                    let mut w = std::io::BufWriter::with_capacity(IO_CHUNK, f);
                    let r = copy_stream(&mut entry, &mut w, &progress);
                    let _ = w.flush();
                    r
                }
                Err(e) => Err(format!("{}: cannot create file: {}", path, e)),
            }
        };

        match written {
            Ok(n) => {
                count += 1;
                total += n;
                progress.add_file();
                if !testing {
                    let mt = entry.header().mtime().unwrap_or(0) as i64;
                    if mt > 0 {
                        let ft = filetime::FileTime::from_unix_time(mt, 0);
                        let _ = filetime::set_file_mtime(&dest, ft);
                    }
                }
                if opts.verbose {
                    println!("  {}", path);
                }
            }
            Err(e) => errors.push(e),
        }
    }

    progress.stop();
    if let Some(h) = bar {
        let _ = h.join();
    }

    if listing {
        println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
        println!("{:>14}  {} files", fmt_size(total), count);
        return common::EXIT_OK;
    }

    for e in &errors {
        eprintln!("fzip: ERROR: {}", e);
    }
    for s in &skipped_unsafe {
        eprintln!("fzip: SKIPPED (unsafe): {}", s);
    }

    if !opts.quiet {
        let secs = t0.elapsed().as_secs_f64();
        let speed = if secs > 0.0 { (total as f64 / secs) as u64 } else { 0 };
        println!("{}", if testing {
            format!("OK: {} files verified, {} in {:.3}s ({}/s)",
                count, fmt_size(total), secs, fmt_size(speed))
        } else {
            format!("Done: {} files, {} in {:.3}s ({}/s)",
                count, fmt_size(total), secs, fmt_size(speed))
        });
        if filtered > 0 {
            println!("  {} entries excluded by filters", filtered);
        }
    }

    if !errors.is_empty() {
        common::EXIT_FATAL
    } else if !skipped_unsafe.is_empty() {
        common::EXIT_WARNING
    } else {
        common::EXIT_OK
    }
}

fn copy_stream<R: Read + ?Sized, W: Write + ?Sized>(
    r: &mut R,
    w: &mut W,
    progress: &Progress,
) -> Result<u64, String> {
    let mut buf = vec![0u8; IO_CHUNK];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("read error: {}", e))?;
        if n == 0 {
            return Ok(total);
        }
        w.write_all(&buf[..n]).map_err(|e| format!("write error: {}", e))?;
        total += n as u64;
        progress.add_bytes(n as u64);
    }
}

/// A single compressed file such as report.txt.gz, with no tar inside.
fn single_file(
    opts: &Options,
    kind: Kind,
    mut stream: Box<dyn Read>,
    mode: Mode,
    t0: Instant,
) -> i32 {
    let name = single_name(&opts.archive, kind);

    if mode == Mode::List {
        println!("{:>14}  {:<10} Name", "Size", "Type");
        println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
        println!("{:>14}  {:<10} {}", "?", kind.label(), name);
        println!("\n(size unknown until decompressed)");
        return common::EXIT_OK;
    }

    let testing = mode == Mode::Test;
    let show_bar = common::progress_enabled(opts.force_progress, opts.quiet, opts.verbose);
    let progress = Progress::new(0, 1, show_bar);
    if !opts.quiet {
        println!("Decompressing {} ({})...",
                           opts.archive.display(), kind.label());
    }
    let bar = progress.spawn(t0);

    let result = if testing {
        copy_stream(&mut stream, &mut std::io::sink(), &progress)
    } else {
        let want = opts.resolve_out_dir();
        match safepath::prepare_root(&want) {
            Ok(root) => {
                let dest = root.join(&name);
                match fs::File::create(&dest) {
                    Ok(f) => {
                        let mut w = std::io::BufWriter::with_capacity(IO_CHUNK, f);
                        let r = copy_stream(&mut stream, &mut w, &progress);
                        let _ = w.flush();
                        r
                    }
                    Err(e) => Err(format!("cannot create {}: {}",
                                      dest.display(), e)),
                }
            }
            Err(e) => Err(format!("cannot create output folder: {}", e)),
        }
    };

    progress.stop();
    if let Some(h) = bar {
        let _ = h.join();
    }

    match result {
        Ok(n) => {
            if !opts.quiet {
                let secs = t0.elapsed().as_secs_f64();
                let speed = if secs > 0.0 { (n as f64 / secs) as u64 } else { 0 };
                println!("Done: {} -> {} in {:.3}s ({}/s)",
                                   name, fmt_size(n), secs, fmt_size(speed));
            }
            common::EXIT_OK
        }
        Err(e) => {
            eprintln!("fzip: ERROR: {}", e);
            common::EXIT_FATAL
        }
    }
}

// ---------------- 7z ----------------

/// What one block produced. Collected per thread, then merged.
#[derive(Default)]
struct BlockOutcome {
    errors: Vec<String>,
    skipped: Vec<String>,
    filtered: usize,
    needs_password: bool,
}

impl BlockOutcome {
    fn absorb(&mut self, other: BlockOutcome) {
        self.errors.extend(other.errors);
        self.skipped.extend(other.skipped);
        self.filtered += other.filtered;
        self.needs_password |= other.needs_password;
    }
}

/// Decode every block of a 7z archive IN PARALLEL, one core per block.
///
/// A 7z archive is divided into independent compression blocks, so each can be
/// decoded on its own thread with its own file handle. The upstream reader
/// walks blocks strictly sequentially, which leaves most of the CPU idle.
/// Fully solid archives have a single block and cannot benefit — there we hand
/// the cores to the LZMA2 decoder instead.
fn extract_7z_blocks(
    archive: &sevenz_rust2::Archive,
    opts: &Options,
    root: &Path,
    password: Option<&str>,
    progress: &Progress,
    testing: bool,
) -> BlockOutcome {
    use sevenz_rust2::{BlockDecoder, Password};

    let n_blocks = archive.blocks.len();
    let cores = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(1);
    // Do not oversubscribe: parallel across blocks OR inside one block, not both.
    let inner_threads = if n_blocks > 1 { 1 } else { cores.max(1) } as u32;

    let per_block: Vec<BlockOutcome> = (0..n_blocks)
        .into_par_iter()
        .map(|bi| {
            let mut out = BlockOutcome::default();
            let pw = match password {
                Some(p) => Password::from(p),
                None => Password::empty(),
            };
            let file = match fs::File::open(&opts.archive) {
                Ok(f) => f,
                Err(e) => {
                    out.errors.push(format!("cannot reopen archive: {}", e));
                    return out;
                }
            };
            let mut src = std::io::BufReader::with_capacity(IO_CHUNK, file);
            let decoder = BlockDecoder::new(inner_threads, bi, archive, &pw, &mut src);

            let res = decoder.for_each_entries(&mut |entry, rd| {
                write_7z_entry(entry, rd, opts, root, progress, testing, &mut out);
                Ok(true)
            });

            if let Err(e) = res {
                match e {
                    sevenz_rust2::Error::PasswordRequired
                    | sevenz_rust2::Error::MaybeBadPassword(_) => {
                        out.needs_password = true;
                        out.errors.push(if password.is_some() {
                            "wrong password".to_string()
                        } else {
                            "archive is encrypted - use -p <password>".to_string()
                        });
                    }
                    sevenz_rust2::Error::ChecksumVerificationFailed => {
                        out.errors.push("checksum failed - archive is corrupt".to_string())
                    }
                    other => out.errors.push(format!("7z error: {}", other)),
                }
            }
            out
        })
        .collect();

    let mut merged = BlockOutcome::default();
    for o in per_block {
        merged.absorb(o);
    }
    merged
}

/// Write one decoded 7z entry, applying filters and path safety.
fn write_7z_entry(
    entry: &sevenz_rust2::ArchiveEntry,
    rd: &mut dyn Read,
    opts: &Options,
    root: &Path,
    progress: &Progress,
    testing: bool,
    out: &mut BlockOutcome,
) {
    let name = entry.name.clone();
    if !opts.filter.matches(&name) {
        out.filtered += 1;
        // The stream must still be drained to keep the block in sync.
        let _ = copy_stream(rd, &mut std::io::sink(), &Progress::new(0, 0, false));
        return;
    }
    if entry.is_directory {
        return;
    }

    let rel = match safepath::sanitize(&name) {
        Ok(r) => r,
        Err(Reject::Traversal) => {
            out.skipped.push(format!("{} (path escapes target folder)", name));
            let _ = copy_stream(rd, &mut std::io::sink(), &Progress::new(0, 0, false));
            return;
        }
        Err(Reject::Empty) => return,
    };
    let rel = if opts.flatten {
        match rel.file_name() {
            Some(n) => PathBuf::from(n),
            None => return,
        }
    } else {
        rel
    };

    progress.set_current(&name);
    let outcome = if testing {
        copy_stream(rd, &mut std::io::sink(), progress)
    } else {
        let dest = root.join(&rel);
        if let Some(p) = dest.parent() {
            let _ = fs::create_dir_all(p);
        }
        match fs::File::create(&dest) {
            Ok(f) => {
                let mut w = std::io::BufWriter::with_capacity(IO_CHUNK, f);
                let r = copy_stream(rd, &mut w, progress);
                let _ = w.flush();
                r
            }
            Err(e) => Err(format!("{}: cannot create file: {}", name, e)),
        }
    };

    match outcome {
        Ok(_) => {
            progress.add_file();
            if opts.verbose {
                println!("  {}", name);
            }
        }
        Err(e) => out.errors.push(e),
    }
}

fn run_7z(opts: &Options, mode: Mode) -> i32 {
    use sevenz_rust2::{Archive, Password};

    let t0 = Instant::now();
    let listing = mode == Mode::List;
    let testing = mode == Mode::Test;
    let mut password = opts.password.clone();

    // Read the header once. Encrypted headers (-mhe) fail here and prompt.
    let archive = loop {
        let pw = match &password {
            Some(p) => Password::from(p.as_str()),
            None => Password::empty(),
        };
        match Archive::open_with_password(&opts.archive, &pw) {
            Ok(a) => break a,
            Err(e) => {
                let needs_pw = matches!(
                    e,
                    sevenz_rust2::Error::PasswordRequired
                        | sevenz_rust2::Error::MaybeBadPassword(_)
                );
                if needs_pw && password.is_none() {
                    match crate::cli::ask_password() {
                        Some(p) => {
                            password = Some(p);
                            continue;
                        }
                        None => {
                            eprintln!("fzip: archive is encrypted - use -p <password>");
                            return common::EXIT_FATAL;
                        }
                    }
                }
                if needs_pw {
                    eprintln!("fzip: wrong password");
                    return common::EXIT_FATAL;
                }
                eprintln!("fzip: cannot open 7z archive: {}", e);
                return common::EXIT_FATAL;
            }
        }
    };

    let entries: Vec<(String, u64, bool)> = archive
        .files
        .iter()
        .map(|f| (f.name.clone(), f.size, f.is_directory))
        .collect();

    if listing {
        println!("{:>14}  {:<10} Name", "Size", "Type");
        println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
        let mut n = 0u64;
        let mut total = 0u64;
        for (name, size, is_dir) in &entries {
            if !opts.filter.matches(name) {
                continue;
            }
            println!("{:>14}  {:<10} {}", size,
                     if *is_dir { "dir" } else { "file" }, name);
            if !*is_dir {
                n += 1;
                total += size;
            }
        }
        println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
        println!("{:>14}  {} files", fmt_size(total), n);
        return common::EXIT_OK;
    }

    let root = if testing {
        PathBuf::new()
    } else {
        let want = opts.resolve_out_dir();
        match safepath::prepare_root(&want) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("fzip: cannot create output folder: {}", e);
                return common::EXIT_FATAL;
            }
        }
    };

    let total_bytes: u64 = entries.iter().filter(|e| !e.2).map(|e| e.1).sum();
    let total_files = entries.iter().filter(|e| !e.2).count() as u64;
    let show_bar = common::progress_enabled(opts.force_progress, opts.quiet, opts.verbose);
    let progress = Progress::new(total_bytes, total_files, show_bar);

    if !opts.quiet {
        println!("Extracting 7z {} ({} files, {})...",
                           opts.archive.display(), total_files, fmt_size(total_bytes));
    }
    // Directories and empty files carry no stream, so they are not in any
    // block. Create them up front; the parallel pass only handles real data.
    if !testing {
        for f in &archive.files {
            if !opts.filter.matches(&f.name) {
                continue;
            }
            if let Ok(rel) = safepath::sanitize(&f.name) {
                if f.is_directory {
                    let _ = fs::create_dir_all(root.join(rel));
                } else if !f.has_stream {
                    let dest = root.join(rel);
                    if let Some(p) = dest.parent() {
                        let _ = fs::create_dir_all(p);
                    }
                    let _ = fs::File::create(&dest);
                }
            }
        }
    }

    let bar = progress.spawn(t0);

    let mut result =
        extract_7z_blocks(&archive, opts, &root, password.as_deref(), &progress, testing);

    // Data-only encryption is invisible until decoding starts, so prompt now
    // and retry once rather than failing with a confusing message.
    if result.needs_password && password.is_none() {
        if let Some(p) = crate::cli::ask_password() {
            password = Some(p);
            result =
                extract_7z_blocks(&archive, opts, &root, password.as_deref(), &progress, testing);
        }
    }

    let errors = result.errors;
    let skipped_unsafe = result.skipped;
    let filtered = result.filtered;

    progress.stop();
    if let Some(h) = bar {
        let _ = h.join();
    }

    for e in &errors {
        eprintln!("fzip: ERROR: {}", e);
    }
    for s in &skipped_unsafe {
        eprintln!("fzip: SKIPPED (unsafe): {}", s);
    }

    if !opts.quiet {
        let secs = t0.elapsed().as_secs_f64();
        let done = progress.bytes_done.load(std::sync::atomic::Ordering::Relaxed);
        let ndone = progress.files_done.load(std::sync::atomic::Ordering::Relaxed);
        let speed = if secs > 0.0 { (done as f64 / secs) as u64 } else { 0 };
        println!("{}", if testing {
            format!("OK: {} files verified, {} in {:.3}s ({}/s)",
                ndone, fmt_size(done), secs, fmt_size(speed))
        } else {
            format!("Done: {} files, {} in {:.3}s ({}/s)",
                ndone, fmt_size(done), secs, fmt_size(speed))
        });
        if filtered > 0 {
            println!("  {} entries excluded by filters", filtered);
        }
    }

    if !errors.is_empty() {
        common::EXIT_FATAL
    } else if !skipped_unsafe.is_empty() {
        common::EXIT_WARNING
    } else {
        common::EXIT_OK
    }
}
