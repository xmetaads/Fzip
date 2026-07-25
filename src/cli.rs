//! Command line parsing and help output.

use std::io::IsTerminal;
use std::path::PathBuf;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::common;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Product name as published. The executable stays lowercase `fzip`.
pub const PRODUCT: &str = "Fzip";
pub const VENDOR: &str = "Tcoder LLC";

/// Upper bound for `-t`. Far above any real core count, low enough that a typo
/// cannot exhaust the machine's thread budget.
const MAX_THREADS: usize = 1024;

/// Formats this build can read. RAR is optional at compile time, so the help
/// text must not promise it when the feature was left out.
#[cfg(feature = "rar")]
pub const READS: &str = "zip, rar, 7z, tar, gz, bz2, xz, zst";
#[cfg(not(feature = "rar"))]
pub const READS: &str = "zip, 7z, tar, gz, bz2, xz, zst (built without RAR support)";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Extract,
    List,
    Test,
    Add,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Overwrite {
    /// Overwrite everything (default)
    Always,
    /// Keep existing files
    Skip,
    /// Write as name_1, name_2, ...
    Rename,
    /// Overwrite only when the archived copy is newer
    Newer,
}

/// Name filters from -i / -x.
pub struct Filter {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl Filter {
    fn build(patterns: &[String]) -> Result<Option<GlobSet>, String> {
        if patterns.is_empty() {
            return Ok(None);
        }
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            let g = Glob::new(p).map_err(|e| format!("bad pattern '{}': {}", p, e))?;
            b.add(g);
            // A pattern without a slash should also match at any depth
            if !p.contains('/') && !p.starts_with("**") {
                if let Ok(g2) = Glob::new(&format!("**/{}", p)) {
                    b.add(g2);
                }
            }
        }
        b.build().map(Some).map_err(|e| format!("cannot build filter: {}", e))
    }

    /// Whether an archive entry name is selected.
    pub fn matches(&self, name: &str) -> bool {
        let norm = name.replace('\\', "/");
        let norm = norm.trim_end_matches('/');
        if let Some(ex) = &self.exclude {
            if ex.is_match(norm) {
                return false;
            }
        }
        match &self.include {
            Some(inc) => inc.is_match(norm),
            None => true,
        }
    }
}

pub struct Options {
    pub mode: Mode,
    pub archive: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub password: Option<String>,
    pub threads: usize,
    pub check_crc: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub force_progress: bool,
    pub overwrite: Overwrite,
    pub filter: Filter,
    pub flatten: bool,
    pub level: u8,
    pub max_memory: u64,
    pub assume_yes: bool,
}

impl Options {
    /// Destination folder: -o when given, otherwise a folder named after the
    /// archive, created NEXT TO THE ARCHIVE — not in the current directory.
    /// This is what 7-Zip and WinRAR do, and it is what makes drag-and-drop
    /// onto fzip.exe behave sensibly (the working directory then belongs to
    /// Explorer, not to the archive).
    pub fn resolve_out_dir(&self) -> PathBuf {
        if let Some(d) = &self.out_dir {
            return d.clone();
        }
        let mut stem = self
            .archive
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "fzip_out".into());
        // Peel up to two extensions so data.tar.gz becomes data
        for _ in 0..2 {
            let lower = stem.to_lowercase();
            let known = [
                ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz", ".zst", ".tgz", ".tbz",
                ".tbz2", ".txz", ".tzst",
            ];
            if let Some(k) = known.iter().find(|k| lower.ends_with(**k)) {
                stem.truncate(stem.len() - k.len());
            } else {
                break;
            }
        }
        if stem.is_empty() {
            stem = "fzip_out".into();
        }
        match self.archive.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(stem),
            _ => PathBuf::from(stem),
        }
    }
}

/// Argument error. An empty message means "just print help".
pub struct ParseError(pub String);

pub fn parse() -> Result<Options, ParseError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err(ParseError(String::new()));
    }

    let mut mode: Option<Mode> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut out_dir = None;
    let mut password = None;
    let mut threads = 0usize;
    let mut check_crc = true;
    let mut quiet = false;
    let mut verbose = false;
    let mut force_progress = false;
    let mut overwrite = Overwrite::Always;
    let mut include: Vec<String> = Vec::new();
    let mut exclude: Vec<String> = Vec::new();
    let mut flatten = false;
    let mut level = 5u8;
    let mut max_memory = 1u64 << 30;
    let mut assume_yes = false;
    let mut prompt_password = false;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        let next = |i: &mut usize, what: &str| -> Result<String, ParseError> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| ParseError(format!("missing value for {}", what)))
        };

        match a.as_str() {
            "-h" | "--help" | "/?" => return Err(ParseError(String::new())),
            "-V" | "--version" => {
                println!("{} {}", PRODUCT, VERSION);
                println!("Copyright (c) 2026 {}. MIT licensed.", VENDOR);
                println!("Fast portable archive tool for Windows");
                println!("Reads {}. Writes zip.", READS);
                std::process::exit(common::EXIT_OK);
            }
            "-o" | "--output" => out_dir = Some(PathBuf::from(next(&mut i, "-o")?)),
            "-p" | "--password" => {
                // A bare -p (or -p followed by another option) prompts instead,
                // which keeps the password out of the shell history.
                let has_value = args.get(i + 1).map(|v| !v.starts_with('-')).unwrap_or(false);
                if has_value {
                    i += 1;
                    password = Some(args[i].clone());
                } else {
                    prompt_password = true;
                }
            }
            "-t" | "--threads" => {
                let v = next(&mut i, "-t")?;
                let n: usize = v
                    .parse()
                    .map_err(|_| ParseError(format!("invalid thread count: {}", v)))?;
                // 0 means "let rayon decide". Cap the rest: asking for tens of
                // thousands of threads would exhaust the machine, not speed it up.
                if n > MAX_THREADS {
                    return Err(ParseError(format!(
                        "thread count {} is too high (maximum {})",
                        n, MAX_THREADS
                    )));
                }
                threads = n;
            }
            "-i" | "--include" => include.push(next(&mut i, "-i")?),
            "-x" | "--exclude" => exclude.push(next(&mut i, "-x")?),
            "-y" | "--yes" => assume_yes = true,
            "-e" | "--flat" => flatten = true,
            "-q" | "--quiet" => quiet = true,
            "-v" | "--verbose" => verbose = true,
            "--no-crc" => check_crc = false,
            "--progress" => force_progress = true,
            "--max-memory" => {
                let v = next(&mut i, "--max-memory")?;
                max_memory = parse_size(&v)
                    .ok_or_else(|| ParseError(format!("invalid size: {}", v)))?;
            }
            "--overwrite" | "-ao" => {
                let v = next(&mut i, "--overwrite")?;
                overwrite = match v.to_lowercase().as_str() {
                    "all" | "a" | "always" => Overwrite::Always,
                    "skip" | "s" | "never" => Overwrite::Skip,
                    "rename" | "r" => Overwrite::Rename,
                    "newer" | "u" => Overwrite::Newer,
                    _ => {
                        return Err(ParseError(format!(
                            "unknown overwrite mode '{}' (use all, skip, rename or newer)",
                            v
                        )))
                    }
                };
            }
            "--level" => {
                let v = next(&mut i, "--level")?;
                level = v.parse::<u8>().unwrap_or(5).min(9);
            }
            _ => {
                if let Some(rest) = a.strip_prefix("-mx") {
                    level = rest.trim_start_matches('=').parse::<u8>().unwrap_or(5).min(9);
                } else if let Some(rest) = a.strip_prefix("-p").filter(|r| !r.is_empty()) {
                    // 7-Zip style: -pSECRET attached to the flag
                    password = Some(rest.to_string());
                } else if let Some(rest) = a.strip_prefix("-i!") {
                    include.push(rest.to_string());
                } else if let Some(rest) = a.strip_prefix("-x!") {
                    exclude.push(rest.to_string());
                } else if a.starts_with('-') && a.len() > 1 && mode.is_some() {
                    return Err(ParseError(format!("unknown option: {}", a)));
                } else if mode.is_none() && positional.is_empty() {
                    match a.to_lowercase().as_str() {
                        "x" | "e" | "extract" | "extractl" | "unzip" => mode = Some(Mode::Extract),
                        "l" | "ls" | "list" => mode = Some(Mode::List),
                        "t" | "test" => mode = Some(Mode::Test),
                        "a" | "add" | "c" | "create" => mode = Some(Mode::Add),
                        _ => positional.push(a),
                    }
                } else {
                    positional.push(a);
                }
            }
        }
        i += 1;
    }

    let mode = mode.unwrap_or(Mode::Extract);

    if positional.is_empty() {
        return Err(ParseError("no archive specified".into()));
    }
    let archive = PathBuf::from(&positional[0]);
    let inputs: Vec<PathBuf> = positional[1..].iter().map(PathBuf::from).collect();

    if mode != Mode::Add && !archive.is_file() {
        return Err(ParseError(format!("file not found: {}", archive.display())));
    }

    // A bare -p means "ask me now". Failing to read one must be an error:
    // silently creating an UNENCRYPTED archive would be the worst outcome.
    if prompt_password && password.is_none() {
        password = Some(ask_password().ok_or_else(|| {
            ParseError(
                "cannot read a password here (not an interactive terminal); \
                 pass it as -p <password>"
                    .into(),
            )
        })?);
    }

    let filter = Filter {
        include: Filter::build(&include).map_err(ParseError)?,
        exclude: Filter::build(&exclude).map_err(ParseError)?,
    };

    Ok(Options {
        mode, archive, inputs, out_dir, password, threads, check_crc, quiet, verbose,
        force_progress, overwrite, filter, flatten, level, max_memory, assume_yes,
    })
}

/// Parse "512", "64M", "2G" into a byte count.
/// Returns None on overflow rather than silently wrapping to a tiny cap.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1024u64),
        'M' => (&s[..s.len() - 1], 1024 * 1024),
        'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.trim().parse::<u64>().ok()?.checked_mul(mul)
}

/// Prompt for a password without echoing it. None when not on a terminal.
pub fn ask_password() -> Option<String> {
    if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Password: ").ok()
    } else {
        None
    }
}

pub fn print_help() {
    println!("{} {} - fast portable archive tool for Windows (no install needed)",
             PRODUCT, VERSION);
    println!("Published by {}. MIT licensed.", VENDOR);
    println!();
    println!("This is a command line tool. Open cmd.exe or PowerShell and run:");
    println!();
    println!("COMMANDS:");
    println!("  fzip x <archive> [options]        extract");
    println!("  fzip <archive>                    extract (shorthand)");
    println!("  fzip l <archive>                  list contents");
    println!("  fzip t <archive>                  test integrity, write nothing");
    println!("  fzip a <archive.zip> <files...>   create a zip");
    println!();
    println!("READS:  {}", READS);
    println!("WRITES: zip, optionally encrypted with AES-256");
    println!();
    println!("OPTIONS:");
    let rows: [(&str, &str); 15] = [
        ("-o <dir>", "output folder (default: archive name)"),
        ("-p <pass>", "password; prompts securely if omitted"),
        ("-t <n>", "CPU threads (default: all)"),
        ("-i <glob>", "include only matching names"),
        ("-x <glob>", "exclude matching names"),
        ("-e", "flatten: ignore folder structure"),
        ("-y", "assume yes (overwrite archive when creating)"),
        ("--overwrite <m>", "all | skip | rename | newer (default: all)"),
        ("-mx<0-9>", "compression level for 'a' (0 = store, 9 = best)"),
        ("--max-memory <n>", "RAM cap, e.g. 512M or 2G (default 1G)"),
        ("--no-crc", "skip CRC verification"),
        ("--progress", "force the progress bar even when redirected"),
        ("-q", "quiet: errors only"),
        ("-v", "verbose: list every file"),
        ("-V", "show version"),
    ];
    for (flag, desc) in rows {
        println!("  {:<18} {}", flag, desc);
    }
    println!();
    println!("EXAMPLES:");
    println!("  fzip x data.zip -o D:\\out");
    println!("  fzip x secret.7z -p MyPass123");
    println!("  fzip x big.rar -x \"*.tmp\" --overwrite skip");
    println!("  fzip a backup.zip photos docs -mx9 -p MyPass123");
    println!("  fzip t archive.zip");
    println!();
    println!("Tip: you can also drag a .zip or .rar file onto fzip.exe.");
    println!();
    println!("EXIT CODES: 0 = ok, 1 = warning, 2 = error, 7 = bad command line");
}
