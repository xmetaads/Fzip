//! Safe path handling: zip-slip defence, Windows device names, long paths.

use std::path::{Path, PathBuf};

/// Reserved Windows device names. A file actually named `CON` or `NUL` is
/// nearly impossible to delete from Explorer, so these are always renamed.
const RESERVED: [&str; 30] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    "LPT0", "CONIN$", "CONOUT$", "COM\u{00b9}", "COM\u{00b2}", "COM\u{00b3}", "CLOCK$",
];

/// Why an archive entry was rejected.
#[derive(Debug, PartialEq)]
pub enum Reject {
    /// Contains `..` — a zip-slip attempt.
    Traversal,
    /// Nothing usable left after cleaning.
    Empty,
}

fn is_reserved(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    RESERVED.iter().any(|r| *r == upper)
}

/// Clean a single path component for Windows.
fn clean_component(part: &str) -> String {
    let mut cleaned: String = part
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();

    // Windows silently strips trailing spaces and dots; append '_' to keep the
    // name distinct instead of letting two entries collide.
    let trimmed = cleaned.trim_end_matches([' ', '.']);
    if trimmed.len() != cleaned.len() {
        cleaned = if trimmed.is_empty() {
            String::new()
        } else {
            format!("{}_", trimmed)
        };
    }

    // Device names are reserved even with an extension, e.g. CON.txt
    let stem = cleaned.split('.').next().unwrap_or("");
    if !stem.is_empty() && is_reserved(stem) {
        cleaned = format!("_{}", cleaned);
    }
    cleaned
}

/// Turn an archive entry name into a safe relative path.
///
/// Blocks `..` traversal, absolute paths, drive letters, forbidden characters
/// and reserved device names.
pub fn sanitize(name: &str) -> Result<PathBuf, Reject> {
    let mut out = PathBuf::new();
    let mut any = false;

    for raw in name.split(['/', '\\']) {
        if raw.is_empty() || raw == "." {
            continue;
        }
        if raw == ".." {
            return Err(Reject::Traversal);
        }
        // Drop a leading drive specifier such as "C:"
        if raw.len() == 2 && raw.ends_with(':') && raw.as_bytes()[0].is_ascii_alphabetic() {
            continue;
        }
        let cleaned = clean_component(raw);
        if cleaned.is_empty() {
            continue;
        }
        out.push(cleaned);
        any = true;
    }

    if any { Ok(out) } else { Err(Reject::Empty) }
}

/// Create the destination folder and return it in extended form (`\\?\C:\...`)
/// so paths longer than the 260-character MAX_PATH limit still work.
pub fn prepare_root(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    // On Windows canonicalize() returns the `\\?\` form, which is what we want.
    match std::fs::canonicalize(dir) {
        Ok(c) => Ok(c),
        Err(_) => Ok(dir.to_path_buf()),
    }
}

/// Strip the `\\?\` prefix when showing a path to the user.
pub fn display(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_zip_slip() {
        assert_eq!(sanitize("../evil.txt"), Err(Reject::Traversal));
        assert_eq!(sanitize("a/../../b"), Err(Reject::Traversal));
        assert_eq!(sanitize("..\\evil"), Err(Reject::Traversal));
    }

    #[test]
    fn strips_absolute_paths() {
        assert_eq!(sanitize("C:/Windows/x.txt").unwrap(), PathBuf::from("Windows/x.txt"));
        assert_eq!(sanitize("/etc/passwd").unwrap(), PathBuf::from("etc/passwd"));
    }

    #[test]
    fn renames_device_names() {
        assert_eq!(sanitize("CON").unwrap(), PathBuf::from("_CON"));
        assert_eq!(sanitize("con.txt").unwrap(), PathBuf::from("_con.txt"));
        assert_eq!(sanitize("LPT1.dat").unwrap(), PathBuf::from("_LPT1.dat"));
        assert_eq!(sanitize("NUL").unwrap(), PathBuf::from("_NUL"));
        // Ordinary names that merely start the same are left alone
        assert_eq!(sanitize("console.log").unwrap(), PathBuf::from("console.log"));
    }

    #[test]
    fn cleans_forbidden_characters() {
        assert_eq!(sanitize("a<b>c.txt").unwrap(), PathBuf::from("a_b_c.txt"));
        assert_eq!(sanitize("name.").unwrap(), PathBuf::from("name_"));
        assert_eq!(sanitize("   ").unwrap_err(), Reject::Empty);
    }

    #[test]
    fn keeps_unicode() {
        assert_eq!(
            sanitize("\u{6587}\u{6863}/caf\u{e9}.txt").unwrap(),
            PathBuf::from("\u{6587}\u{6863}/caf\u{e9}.txt")
        );
    }
}
