//! RAR reading, backed by RARLAB's official UnRAR sources.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use unrar::error::Code as RarCode;
use unrar::Archive as RarArchive;

use crate::cli::{Mode, Options};
use crate::common::{self, fmt_size, Progress};
use crate::safepath;

struct Item {
    name: PathBuf,
    size: u64,
    is_file: bool,
    encrypted: bool,
}

fn needs_password(code: RarCode) -> bool {
    matches!(code, RarCode::MissingPassword | RarCode::BadPassword)
}

/// List the archive, prompting for a password if even the headers are
/// encrypted (created with `rar a -hp`).
fn collect(archive: &PathBuf, password: &mut Option<String>) -> Result<Vec<Item>, String> {
    loop {
        let listing = match password.as_deref() {
            Some(p) => RarArchive::with_password(archive, p).open_for_listing(),
            None => RarArchive::new(archive).open_for_listing(),
        };
        let list = match listing {
            Ok(l) => l,
            Err(e) => {
                if needs_password(e.code) && password.is_none() {
                    match crate::cli::ask_password() {
                        Some(pw) => {
                            *password = Some(pw);
                            continue;
                        }
                        None => {
                            return Err("archive is encrypted - use -p <password>".into())
                        }
                    }
                }
                if needs_password(e.code) {
                    return Err("wrong password".into());
                }
                return Err(format!("cannot open RAR archive: {}", e));
            }
        };

        let mut items = Vec::new();
        let mut need_pw = false;
        let mut fail: Option<String> = None;
        for it in list {
            match it {
                Ok(h) => items.push(Item {
                    name: h.filename.clone(),
                    size: h.unpacked_size,
                    is_file: h.is_file(),
                    encrypted: h.is_encrypted(),
                }),
                Err(e) => {
                    if needs_password(e.code) && password.is_none() {
                        need_pw = true;
                    } else {
                        fail = Some(format!("RAR read error: {}", e));
                    }
                    break;
                }
            }
        }
        if need_pw {
            match crate::cli::ask_password() {
                Some(pw) => {
                    *password = Some(pw);
                    continue;
                }
                None => return Err("archive is encrypted - use -p <password>".into()),
            }
        }
        if let Some(f) = fail {
            return Err(f);
        }
        return Ok(items);
    }
}

pub fn run_list(opts: &Options) -> i32 {
    let mut password = opts.password.clone();
    let items = match collect(&opts.archive, &mut password) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };

    println!("{:>14}  {:<10} Name", "Size", "Crypto");
    println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
    let mut total = 0u64;
    let mut n = 0u64;
    for it in &items {
        let name = it.name.to_string_lossy();
        if !it.is_file || !opts.filter.matches(&name) {
            continue;
        }
        n += 1;
        total += it.size;
        println!("{:>14}  {:<10} {}", it.size, if it.encrypted { "RAR" } else { "" }, name);
    }
    println!("{:-<14}  {:-<10} {:-<40}", "", "", "");
    println!("{:>14}  {} files", fmt_size(total), n);
    common::EXIT_OK
}

pub fn run_extract(opts: &Options, mode: Mode) -> i32 {
    let t0 = Instant::now();
    let testing = mode == Mode::Test;
    let mut password = opts.password.clone();

    let items = match collect(&opts.archive, &mut password) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("fzip: {}", e);
            return common::EXIT_FATAL;
        }
    };

    // Entries can be listed even when only the DATA is encrypted, so ask here.
    if password.is_none() && items.iter().any(|i| i.encrypted) {
        match crate::cli::ask_password() {
            Some(pw) => password = Some(pw),
            None => {
                eprintln!("fzip: archive is encrypted - use -p <password>");
                return common::EXIT_FATAL;
            }
        }
    }

    let root = if testing {
        PathBuf::new()
    } else {
        let want = opts.resolve_out_dir();
        match safepath::prepare_root(&want) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("fzip: cannot create output folder {}: {}", want.display(), e);
                return common::EXIT_FATAL;
            }
        }
    };

    let selected: Vec<&Item> = items
        .iter()
        .filter(|i| i.is_file && opts.filter.matches(&i.name.to_string_lossy()))
        .collect();
    let total_files = selected.len() as u64;
    let total_bytes: u64 = selected.iter().map(|i| i.size).sum();
    let filtered = items.iter().filter(|i| i.is_file).count() as u64 - total_files;

    let show_bar = common::progress_enabled(opts.force_progress, opts.quiet, opts.verbose);
    let progress = Progress::new(total_bytes, total_files, show_bar);

    if !opts.quiet {
        if testing {
            println!("Testing RAR {} ({} files, {})...",
                     opts.archive.display(), total_files, fmt_size(total_bytes));
        } else {
            println!("Extracting RAR {} ({} files, {}) -> {}",
                     opts.archive.display(), total_files, fmt_size(total_bytes),
                     safepath::display(&root));
        }
    }
    let bar = progress.spawn(t0);

    let mut errors: Vec<String> = Vec::new();
    let mut skipped_unsafe: Vec<String> = Vec::new();

    let open = match password.as_deref() {
        Some(p) => RarArchive::with_password(&opts.archive, p).open_for_processing(),
        None => RarArchive::new(&opts.archive).open_for_processing(),
    };

    match open {
        Err(e) => errors.push(format!("cannot open RAR archive: {}", e)),
        Ok(mut proc) => loop {
            match proc.read_header() {
                Ok(None) => break,
                Ok(Some(h)) => {
                    let name_s = h.entry().filename.to_string_lossy().into_owned();
                    let size = h.entry().unpacked_size;
                    let is_file = h.entry().is_file();
                    let wanted = is_file && opts.filter.matches(&name_s);

                    // Validate the path BEFORE writing anything
                    let mut unsafe_path = false;
                    let safe_rel = if wanted && !testing {
                        match safepath::sanitize(&name_s) {
                            Ok(r) => {
                                if opts.flatten {
                                    r.file_name().map(PathBuf::from)
                                } else {
                                    Some(r)
                                }
                            }
                            Err(_) => {
                                skipped_unsafe
                                    .push(format!("{} (path escapes target folder)", name_s));
                                unsafe_path = true;
                                None
                            }
                        }
                    } else {
                        None
                    };

                    progress.set_current(&name_s);
                    let step = if !wanted || unsafe_path {
                        h.skip()
                    } else if testing {
                        h.test()
                    } else if let Some(rel) = safe_rel {
                        let dest = root.join(&rel);
                        if let Some(p) = dest.parent() {
                            let _ = fs::create_dir_all(p);
                        }
                        h.extract_to(&dest)
                    } else {
                        h.skip()
                    };

                    match step {
                        Ok(next) => {
                            proc = next;
                            if wanted && !unsafe_path {
                                progress.add_bytes(size);
                                progress.add_file();
                                if opts.verbose {
                                    println!("  {}", name_s);
                                }
                            }
                        }
                        Err(e) => {
                            let hint = match e.code {
                                RarCode::BadPassword => " (wrong password)",
                                RarCode::BadData => " (wrong password or corrupt data)",
                                _ => "",
                            };
                            errors.push(format!("{}: {}{}", name_s, e, hint));
                            break;
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("RAR header error: {}", e));
                    break;
                }
            }
        },
    }

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
        use std::sync::atomic::Ordering;
        let secs = t0.elapsed().as_secs_f64();
        let done = progress.bytes_done.load(Ordering::Relaxed);
        let n = progress.files_done.load(Ordering::Relaxed);
        let speed = if secs > 0.0 { (done as f64 / secs) as u64 } else { 0 };
        if testing {
            println!("OK: {} files verified, {} in {:.3}s ({}/s)",
                     n, fmt_size(done), secs, fmt_size(speed));
        } else {
            println!("Done: {} files, {} in {:.3}s ({}/s)",
                     n, fmt_size(done), secs, fmt_size(speed));
        }
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
