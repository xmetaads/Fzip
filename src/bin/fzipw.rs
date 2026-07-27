//! `fzipw.exe` - the windowless build, for installers and scheduled tasks.
//!
//! Identical to `fzip.exe` in every respect except one field in the PE header:
//! the subsystem is `WINDOWS` rather than `CONSOLE`. That single flag is what
//! stops Windows allocating a console, which is what stops the black window
//! flashing when an MSI custom action, a scheduled task or a service launches
//! Fzip.
//!
//! The convention is Windows' own: `python.exe` / `pythonw.exe`,
//! `java.exe` / `javaw.exe`. A separate binary rather than a changed one,
//! because the flag is not a free win - it is a trade:
//!
//! | Launched by | `fzip.exe` | `fzipw.exe` |
//! |---|---|---|
//! | A person, from a terminal | output, progress bar | output only if redirected |
//! | Double-click in Explorer | help screen, then pauses | nothing visible at all |
//! | MSI custom action, service | **console flashes** | silent, no window, ever |
//!
//! With no console allocated, `GetConsoleWindow()` returns 0 and the standard
//! handles are NULL, so anything printed goes nowhere. Measured, not assumed.
//! Redirect it (`fzipw x a.zip > log.txt 2>&1`) and the output reappears,
//! because the inherited handles are then real. Exit codes work either way, and
//! for an installer the exit code is what matters: 0 ok, 1 warning, 2 error,
//! 7 bad command line.
//!
//! Anything that needs a human to read it should use `fzip.exe`.

#![windows_subsystem = "windows"]

fn main() {
    std::process::exit(fzip::run_cli());
}
