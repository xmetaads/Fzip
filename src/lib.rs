//! Fzip - fast portable zip tool for Windows.
//!
//! Copyright (c) 2026 Tcoder LLC. MIT licensed.
//!
//! Portable in the same sense as Microsoft's azcopy.exe: a single
//! self-contained .exe with nothing to install, driven entirely from cmd.exe or
//! PowerShell. That comparison is about how the tool is shipped, not how it is
//! built - azcopy is written in Go, this is Rust.
//!
//! Speed comes from three things:
//!   1. Memory-mapping the whole archive, so entries are reached without a
//!      read syscall each
//!   2. Decompressing entries IN PARALLEL, one CPU core each (rayon)
//!   3. zune-inflate and libdeflate, the fastest DEFLATE codecs available
//!
//! Reads zip. Writes zip, optionally encrypted with AES-256.
//!
//! # Two executables, one implementation
//!
//! Everything lives here so that `fzip.exe` and `fzipw.exe` are the same program
//! differing only in their PE subsystem field. See `src/bin/` - the difference is
//! two lines, and it matters a great deal to anyone embedding Fzip in an
//! installer.

pub mod cli;
pub mod common;
pub mod crypto;
pub mod safepath;
pub mod zipread;
pub mod zipwrite;

use std::fs;
use std::io::Write;
use std::path::Path;

use cli::Mode;
use memmap2::Mmap;

/// Name an archive Fzip no longer handles, so someone arriving from 1.1 or
/// earlier gets an explanation rather than a complaint about a broken zip.
fn describe_foreign_format(head: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| head.len() >= sig.len() && &head[..sig.len()] == sig;
    if starts(b"Rar!\x1A\x07") {
        Some("a RAR archive")
    } else if starts(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]) {
        Some("a 7z archive")
    } else if starts(&[0x1F, 0x8B]) {
        Some("a gzip file")
    } else if starts(b"BZh") {
        Some("a bzip2 file")
    } else if starts(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        Some("an xz file")
    } else if starts(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Some("a zstd file")
    } else if head.len() > 262 && &head[257..262] == b"ustar" {
        Some("a tar archive")
    } else {
        None
    }
}

/// Keep the window open when launched by double-click, like azcopy.exe does.
///
/// In `fzipw.exe` this is inert without a special case: with no console
/// allocated, `GetConsoleProcessList` reports none owned and `should_pause`
/// returns false. The silent build cannot hang waiting for a keypress even if
/// someone forgets `--no-pause`.
fn pause_if_own_console(no_pause: bool) {
    if common::should_pause(no_pause) {
        print!("\nPress Enter to exit...");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }
}

/// The whole program, from argument parsing to exit code. Both executables call
/// this and nothing else, so the two can never drift apart in behaviour.
pub fn run_cli() -> i32 {
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
            // Argument parsing failed, so no --no-pause was captured; the
            // environment variable is still honoured inside should_pause.
            pause_if_own_console(false);
            return code;
        }
    };

    if opts.threads > 0 {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(opts.threads)
            .build_global();
    }

    let code = run(&opts);
    pause_if_own_console(opts.no_pause);
    code
}

fn run(opts: &cli::Options) -> i32 {
    // Creating an archive needs no format detection.
    if opts.mode == Mode::Add {
        let looks_like_zip = opts
            .archive
            .extension()
            .map(|e| e.eq_ignore_ascii_case("zip"))
            .unwrap_or(false);
        if !looks_like_zip && !opts.quiet {
            eprintln!(
                "fzip: warning: {} does not end in .zip, but a zip is what will be written",
                opts.archive.display()
            );
        }
        return zipwrite::run_add(opts);
    }

    let file = match fs::File::open(&opts.archive) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("fzip: cannot open {}: {}", opts.archive.display(), e);
            return common::EXIT_FATAL;
        }
    };

    // A zero-byte file cannot be mapped, and is not a zip either way.
    if file.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
        eprintln!("fzip: {} is empty, not a zip file", opts.archive.display());
        return common::EXIT_FATAL;
    }

    // SAFETY: the mapping is read-only and lives no longer than `file`. A
    // concurrent truncation by another process would fault, which is the same
    // exposure every memory-mapping archiver carries.
    let mmap = match unsafe { Mmap::map(&file) } {
        Ok(m) => m,
        Err(e) => {
            eprintln!("fzip: cannot read {}: {}", opts.archive.display(), e);
            return common::EXIT_FATAL;
        }
    };

    if let Some(code) = reject_non_zip(&mmap, &opts.archive) {
        return code;
    }

    match opts.mode {
        Mode::List => zipread::run_list(opts, &mmap),
        m => zipread::run_extract(opts, &mmap, m),
    }
}

/// Identify by magic bytes, never by extension, so a misnamed zip still opens
/// and a genuinely foreign archive is named rather than mis-parsed.
fn reject_non_zip(data: &[u8], path: &Path) -> Option<i32> {
    if data.starts_with(b"PK") {
        return None;
    }
    match describe_foreign_format(data) {
        Some(what) => eprintln!(
            "fzip: {} is {}, and this version reads zip only",
            path.display(),
            what
        ),
        None => eprintln!("fzip: {} is not a valid zip file", path.display()),
    }
    Some(common::EXIT_FATAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_formats_this_version_dropped() {
        assert_eq!(describe_foreign_format(b"Rar!\x1A\x07\x01\x00"), Some("a RAR archive"));
        assert_eq!(
            describe_foreign_format(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C, 0]),
            Some("a 7z archive")
        );
        assert_eq!(describe_foreign_format(&[0x1F, 0x8B, 0x08]), Some("a gzip file"));
        assert_eq!(describe_foreign_format(b"BZh9"), Some("a bzip2 file"));
        // A zip is not foreign, and neither is arbitrary rubbish.
        assert_eq!(describe_foreign_format(b"PK\x03\x04"), None);
        assert_eq!(describe_foreign_format(b"hello"), None);
        // A short buffer must not index out of range.
        assert_eq!(describe_foreign_format(b""), None);
        assert_eq!(describe_foreign_format(b"R"), None);
    }
}
