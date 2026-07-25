//! Shared foundations: console setup, time conversion, progress reporting.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------- Windows console ----------------

/// Switch the console to UTF-8 so non-ASCII file names render correctly.
#[cfg(windows)]
pub fn init_console() {
    extern "system" {
        fn SetConsoleOutputCP(cp: u32) -> i32;
        fn GetConsoleMode(h: isize, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: isize, mode: u32) -> i32;
        fn GetStdHandle(n: i32) -> isize;
    }
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    unsafe {
        SetConsoleOutputCP(65001);
        let h = GetStdHandle(-11); // STD_OUTPUT_HANDLE
        let mut mode = 0u32;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

#[cfg(not(windows))]
pub fn init_console() {}

/// True when this process OWNS the console window, i.e. the user launched it
/// by double-clicking from Explorer rather than from an existing shell.
#[cfg(windows)]
pub fn owns_console() -> bool {
    extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
    }
    let mut buf = [0u32; 4];
    unsafe { GetConsoleProcessList(buf.as_mut_ptr(), buf.len() as u32) == 1 }
}

#[cfg(not(windows))]
pub fn owns_console() -> bool {
    false
}

/// Whether there is a console window a person could actually be looking at.
///
/// An installer that launches fzip hidden still gets a console allocated, and
/// that console still reports itself as a terminal — so owning it, or stdin
/// being a tty, is not enough to conclude anyone is watching. The window
/// itself being visible is. Without this check a hidden run would stop at
/// "Press Enter to exit..." and wait forever, with no keyboard to answer it.
#[cfg(windows)]
pub fn console_is_visible() -> bool {
    extern "system" {
        fn GetConsoleWindow() -> isize;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
    }
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd == 0 {
            return false;
        }
        // Minimised still counts as visible: the user can restore it.
        IsWindowVisible(hwnd) != 0 || IsIconic(hwnd) != 0
    }
}

#[cfg(not(windows))]
pub fn console_is_visible() -> bool {
    true
}

/// Whether to hold the window open at the end so a double-click user can read
/// the output. Every condition must hold, because getting this wrong means an
/// unattended run hangs forever instead of finishing.
pub fn should_pause(no_pause_flag: bool) -> bool {
    if no_pause_flag || std::env::var_os("FZIP_NO_PAUSE").is_some() {
        return false;
    }
    owns_console() && console_is_visible() && std::io::stdin().is_terminal()
}

// ---------------- time ----------------

/// Local timezone offset from UTC in seconds.
/// ZIP stores timestamps in LOCAL time, so this is needed for correct dates.
#[cfg(windows)]
pub fn local_offset_secs() -> i64 {
    static CACHE: OnceLock<i64> = OnceLock::new();
    *CACHE.get_or_init(|| {
        #[repr(C)]
        #[derive(Default)]
        struct SysTime {
            year: u16,
            month: u16,
            dow: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            ms: u16,
        }
        extern "system" {
            fn GetLocalTime(p: *mut SysTime);
            fn GetSystemTime(p: *mut SysTime);
        }
        let mut l = SysTime::default();
        let mut u = SysTime::default();
        unsafe {
            GetLocalTime(&mut l);
            GetSystemTime(&mut u);
        }
        let to_secs = |s: &SysTime| {
            days_from_civil(s.year as i64, s.month as i64, s.day as i64) * 86_400
                + s.hour as i64 * 3600
                + s.minute as i64 * 60
                + s.second as i64
        };
        // Round to whole minutes to land exactly on a standard UTC offset
        let diff = to_secs(&l) - to_secs(&u);
        (diff as f64 / 60.0).round() as i64 * 60
    })
}

#[cfg(not(windows))]
pub fn local_offset_secs() -> i64 {
    0
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_round_trip_keeps_the_moment() {
        // DOS stores seconds in 2-second units, so only even seconds survive.
        for &unix in &[
            315_532_800, // 1980-01-01
            1_751_371_200, // 2025-07-01, inside DST for northern zones
            1_735_689_600, // 2025-01-01, outside DST
            2_000_000_000,
        ] {
            let (d, t) = unix_to_dos(unix);
            let back = dos_to_unix(d, t).expect("packed value must decode");
            assert_eq!(back, unix, "round trip drifted for {}", unix);
        }
    }

    #[test]
    fn dos_rejects_nonsense_fields() {
        // hour 31 / minute 63 / second 62 come straight from 0xFFFF
        assert_eq!(dos_to_unix(0x21, 0xFFFF), None);
        assert_eq!(dos_to_unix(0, 0), None); // day 0
    }

    #[test]
    fn pre_1980_clamps_to_the_start_of_the_range() {
        let (d, t) = pack_dos(1975, 6, 15, 12, 30, 0);
        assert_eq!((d, t), ((1 << 5) | 1, 0), "must be 1980-01-01, not 1980-06-15");
    }
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Convert between UTC and local time *for the given moment*, so summer and
/// winter timestamps each use their own offset.
///
/// The cached `local_offset_secs()` delta is only correct for "now": applying
/// today's winter offset to a July timestamp shifts it by the DST amount, which
/// makes fzip disagree with every other archiver for half the year and drift by
/// an hour on every round trip. Windows exposes the per-date rules directly.
#[cfg(windows)]
mod tz {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct SystemTime {
        pub year: u16,
        pub month: u16,
        pub dow: u16,
        pub day: u16,
        pub hour: u16,
        pub minute: u16,
        pub second: u16,
        pub ms: u16,
    }

    extern "system" {
        fn SystemTimeToTzSpecificLocalTime(tz: *const u8, utc: *const SystemTime,
                                           local: *mut SystemTime) -> i32;
        fn TzSpecificLocalTimeToSystemTime(tz: *const u8, local: *const SystemTime,
                                           utc: *mut SystemTime) -> i32;
    }

    pub fn utc_to_local(utc: SystemTime) -> Option<SystemTime> {
        let mut out = SystemTime::default();
        let ok = unsafe {
            SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut out)
        };
        if ok != 0 { Some(out) } else { None }
    }

    pub fn local_to_utc(local: SystemTime) -> Option<SystemTime> {
        let mut out = SystemTime::default();
        let ok = unsafe {
            TzSpecificLocalTimeToSystemTime(std::ptr::null(), &local, &mut out)
        };
        if ok != 0 { Some(out) } else { None }
    }
}

#[cfg(windows)]
fn split_unix(unix: i64) -> tz::SystemTime {
    let days = unix.div_euclid(86_400);
    let rem = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    tz::SystemTime {
        year: y.clamp(1601, 30827) as u16,
        month: m as u16,
        dow: 0,
        day: d as u16,
        hour: (rem / 3600) as u16,
        minute: ((rem % 3600) / 60) as u16,
        second: (rem % 60) as u16,
        ms: 0,
    }
}

#[cfg(windows)]
fn join_unix(t: &tz::SystemTime) -> i64 {
    days_from_civil(t.year as i64, t.month as i64, t.day as i64) * 86_400
        + t.hour as i64 * 3600
        + t.minute as i64 * 60
        + t.second as i64
}

/// DOS date/time (local) -> Unix timestamp (UTC)
pub fn dos_to_unix(date: u16, time: u16) -> Option<i64> {
    let day = (date & 31) as i64;
    let month = ((date >> 5) & 15) as i64;
    let year = 1980 + ((date >> 9) & 127) as i64;
    let sec = ((time & 31) * 2) as i64;
    let min = ((time >> 5) & 63) as i64;
    let hour = ((time >> 11) & 31) as i64;
    // Every field is attacker-controlled; reject nonsense rather than inventing
    // an mtime of "hour 31".
    if day == 0 || !(1..=12).contains(&month) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }

    #[cfg(windows)]
    {
        let local = tz::SystemTime {
            year: year as u16,
            month: month as u16,
            dow: 0,
            day: day as u16,
            hour: hour as u16,
            minute: min as u16,
            second: sec as u16,
            ms: 0,
        };
        if let Some(utc) = tz::local_to_utc(local) {
            return Some(join_unix(&utc));
        }
    }

    let local = days_from_civil(year, month, day) * 86_400 + hour * 3600 + min * 60 + sec;
    Some(local - local_offset_secs())
}

/// Unix timestamp (UTC) -> DOS date/time (local)
pub fn unix_to_dos(unix: i64) -> (u16, u16) {
    #[cfg(windows)]
    {
        if let Some(l) = tz::utc_to_local(split_unix(unix)) {
            return pack_dos(l.year as i64, l.month as i64, l.day as i64,
                            l.hour as i64, l.minute as i64, l.second as i64);
        }
    }

    let local = unix + local_offset_secs();
    let days = local.div_euclid(86_400);
    let rem = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    pack_dos(y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Pack a local civil time into the DOS date/time pair. DOS cannot represent
/// anything before 1980, so such timestamps clamp to the very start of its range
/// rather than keeping a month and day that no longer belong to the year.
fn pack_dos(y: i64, m: i64, d: i64, hour: i64, min: i64, sec: i64) -> (u16, u16) {
    if y < 1980 {
        return ((1 << 5) | 1, 0); // 1980-01-01 00:00
    }
    if y > 2107 {
        return ((127 << 9) | (12 << 5) | 31, (23 << 11) | (59 << 5) | 29);
    }
    let date = (((y - 1980) as u16) << 9) | ((m as u16) << 5) | (d as u16);
    let time = ((hour as u16) << 11) | ((min as u16) << 5) | ((sec / 2) as u16);
    (date, time)
}

// ---------------- formatting ----------------

pub fn fmt_size(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", n, UNITS[u])
    } else {
        format!("{:.2} {}", v, UNITS[u])
    }
}

pub fn fmt_duration(secs: f64) -> String {
    if secs < 60.0 {
        format!("{:.0}s", secs)
    } else if secs < 3600.0 {
        format!("{:.0}m{:02.0}s", secs / 60.0, secs % 60.0)
    } else {
        format!("{:.0}h{:02.0}m", secs / 3600.0, (secs % 3600.0) / 60.0)
    }
}

// ---------------- progress bar ----------------

pub struct Progress {
    pub bytes_done: AtomicU64,
    pub files_done: AtomicU64,
    pub total_bytes: u64,
    pub total_files: u64,
    pub finished: AtomicBool,
    pub current: Mutex<String>,
    enabled: bool,
}

impl Progress {
    pub fn new(total_bytes: u64, total_files: u64, enabled: bool) -> Arc<Self> {
        Arc::new(Progress {
            bytes_done: AtomicU64::new(0),
            files_done: AtomicU64::new(0),
            total_bytes,
            total_files,
            finished: AtomicBool::new(false),
            current: Mutex::new(String::new()),
            enabled,
        })
    }

    pub fn set_current(&self, name: &str) {
        if self.enabled {
            if let Ok(mut g) = self.current.lock() {
                g.clear();
                g.push_str(name);
            }
        }
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_file(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, start: Instant, last_len: &mut usize) {
        let done = self.bytes_done.load(Ordering::Relaxed);
        let files = self.files_done.load(Ordering::Relaxed);
        let secs = start.elapsed().as_secs_f64();
        let speed = if secs > 0.01 { done as f64 / secs } else { 0.0 };

        // Nothing to draw a bar against - an archive with no entries at all.
        // Show a plain activity line rather than dividing by zero.
        let line = if self.total_bytes == 0 && self.total_files == 0 {
            format!(
                "\r  {} files  {}  {}/s",
                files,
                fmt_size(done),
                fmt_size(speed as u64)
            )
        } else {
            let pct = if self.total_bytes > 0 {
                (done as f64 / self.total_bytes as f64 * 100.0).min(100.0)
            } else {
                (files as f64 / self.total_files as f64 * 100.0).min(100.0)
            };
            let eta = if speed > 1.0 && self.total_bytes > done {
                fmt_duration((self.total_bytes - done) as f64 / speed)
            } else {
                "--".to_string()
            };
            const W: usize = 24;
            let filled = ((pct / 100.0) * W as f64) as usize;
            let mut bar = String::with_capacity(W);
            for i in 0..W {
                bar.push(if i < filled {
                    '='
                } else if i == filled && pct < 100.0 {
                    '>'
                } else {
                    ' '
                });
            }
            format!(
                "\r[{}] {:5.1}%  {}/{}  {}  {}/s  ETA {}",
                bar,
                pct,
                files,
                self.total_files,
                fmt_size(done),
                fmt_size(speed as u64),
                eta
            )
        };

        let pad = last_len.saturating_sub(line.chars().count());
        *last_len = line.chars().count();
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(line.as_bytes());
        for _ in 0..pad {
            let _ = out.write_all(b" ");
        }
        let _ = out.flush();
    }

    /// Start the progress-rendering thread. Returns None when disabled.
    pub fn spawn(self: &Arc<Self>, start: Instant) -> Option<std::thread::JoinHandle<()>> {
        if !self.enabled {
            return None;
        }
        let p = self.clone();
        Some(std::thread::spawn(move || {
            let mut last_len = 0usize;
            while !p.finished.load(Ordering::Relaxed) {
                p.render(start, &mut last_len);
                std::thread::sleep(Duration::from_millis(100));
            }
            p.render(start, &mut last_len);
            println!();
        }))
    }

    pub fn stop(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }
}

/// Only draw a progress bar on a real terminal, so logs and pipes stay clean.
pub fn progress_enabled(force: bool, quiet: bool, verbose: bool) -> bool {
    !quiet && !verbose && (force || std::io::stdout().is_terminal())
}

// ---------------- exit codes (follows the 7-Zip convention) ----------------

pub const EXIT_OK: i32 = 0;
pub const EXIT_WARNING: i32 = 1;
pub const EXIT_FATAL: i32 = 2;
pub const EXIT_USAGE: i32 = 7;
