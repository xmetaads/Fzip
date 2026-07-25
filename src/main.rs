//! Fzip - fast portable archive tool for Windows.
//!
//! Copyright (c) 2026 Tcoder LLC. MIT licensed.
//!
//! Portable in the same sense as Microsoft's azcopy.exe: a single
//! self-contained .exe with nothing to install. That is a comparison of
//! distribution model only - azcopy is written in Go, this is Rust.
//!
//! Speed comes from three things:
//!   1. Memory-mapping the whole archive (no read syscalls)
//!   2. Decompressing entries IN PARALLEL, one CPU core each (rayon)
//!   3. zune-inflate / libdeflate, the fastest DEFLATE codecs available
//!
//! Reads zip, rar, 7z, tar, gz, bz2, xz, zst. Writes zip, optionally AES-256.

mod cli;
mod common;
mod crypto;
mod otherfmt;
#[cfg(feature = "rar")]
mod rarfmt;
mod safepath;
mod zipread;
mod zipwrite;

use std::fs;
use std::io::{Read, Write};

use cli::Mode;
use memmap2::Mmap;

#[derive(PartialEq, Clone, Copy, Debug)]
enum Format {
    Zip,
    Rar,
    Other(otherfmt::Kind),
}

/// Identify the format from the file's magic bytes; the extension is only a
/// last resort, so a misnamed archive still extracts correctly.
fn detect_format(path: &std::path::Path) -> Format {
    let mut head = [0u8; 512];
    let n = fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let sig = &head[..n];

    if n >= 7 && sig.starts_with(b"Rar!\x1A\x07") {
        return Format::Rar;
    }
    if n >= 2 && sig.starts_with(b"PK") {
        return Format::Zip;
    }
    if let Some(k) = otherfmt::detect(sig, sig) {
        return Format::Other(k);
    }

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rar" => Format::Rar,
        "7z" => Format::Other(otherfmt::Kind::SevenZ),
        "tar" => Format::Other(otherfmt::Kind::Tar),
        "gz" | "tgz" => Format::Other(otherfmt::Kind::Gz),
        "bz2" | "tbz" | "tbz2" => Format::Other(otherfmt::Kind::Bz2),
        "xz" | "txz" => Format::Other(otherfmt::Kind::Xz),
        "zst" | "tzst" => Format::Other(otherfmt::Kind::Zst),
        _ => Format::Zip,
    }
}

/// Keep the window open when launched by double-click, like azcopy.exe does.
fn pause_if_own_console() {
    if common::owns_console() {
        print!("\nPress Enter to exit...");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}

fn main() {
    common::init_console();

    let opts = match cli::parse() {
        Ok(o) => o,
        Err(e) => {
            let code = if e.0.is_empty() {
                common::EXIT_OK
            } else {
                eprintln!("fzip: {}", e.0);
                eprintln!();
                common::EXIT_USAGE
            };
            cli::print_help();
            pause_if_own_console();
            std::process::exit(code);
        }
    };

    if opts.threads > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(opts.threads)
            .build_global();
    }

    let code = run(&opts);
    pause_if_own_console();
    std::process::exit(code);
}

fn run(opts: &cli::Options) -> i32 {
    // Creating an archive needs no format detection
    if opts.mode == Mode::Add {
        return zipwrite::run_add(opts);
    }

    match detect_format(&opts.archive) {
        #[cfg(feature = "rar")]
        Format::Rar => match opts.mode {
            Mode::List => rarfmt::run_list(opts),
            m => rarfmt::run_extract(opts, m),
        },
        #[cfg(not(feature = "rar"))]
        Format::Rar => {
            eprintln!("fzip: this build has no RAR support (built without the 'rar' feature)");
            common::EXIT_FATAL
        }
        Format::Other(kind) => otherfmt::run(opts, kind, opts.mode),
        Format::Zip => {
            // ZIP is memory-mapped for maximum throughput
            let file = match fs::File::open(&opts.archive) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("fzip: cannot open {}: {}", opts.archive.display(), e);
                    return common::EXIT_FATAL;
                }
            };
            let mmap = match unsafe { Mmap::map(&file) } {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("fzip: cannot memory-map {}: {}", opts.archive.display(), e);
                    return common::EXIT_FATAL;
                }
            };
            match opts.mode {
                Mode::List => zipread::run_list(opts, &mmap),
                m => zipread::run_extract(opts, &mmap, m),
            }
        }
    }
}
