//! `fzip.exe` - the console build. This is the one people type commands into.
//!
//! Console subsystem, so it keeps a console when launched from Explorer: the
//! help screen appears on a double-click, drag-and-drop shows a progress bar,
//! and errors are visible instead of vanishing.
//!
//! For installers and other unattended callers, use `fzipw.exe` alongside it.

fn main() {
    std::process::exit(fzip::run_cli());
}
