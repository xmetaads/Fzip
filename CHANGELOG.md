# Changelog

All notable changes to **Fzip**, published by Tcoder LLC.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [Semantic Versioning](https://semver.org/).

## [1.0.1] — 2026-07-25

Five defects reported from real deployments, all reproduced and fixed. Anyone
driving Fzip from an installer or a script should take this release.

### Fixed

- **A hidden run hung forever.** An installer launching `fzip.exe` with no
  visible window still gets a console allocated, and Fzip owned it — so it
  reached "Press Enter to exit..." and waited for a keyboard that was not there.
  The process never exited and the installer sat at 100% indefinitely. Fzip now
  checks whether the console window is actually *visible* before pausing;
  owning a console and stdin reporting as a terminal are not enough to conclude
  someone is watching.
- **`--no-pause` and `FZIP_NO_PAUSE`** added so automation can rule the pause
  out explicitly rather than relying on detection.
- **Directory entries ending in a backslash were extracted as files.** The
  specification says `/`, but plenty of Windows producers write `\`. Fzip
  treated `python\lib\venv\scripts\` as a file, tried to create a file over the
  existing folder, and failed with *"cannot create file: Access is denied
  (os error 5)"* — losing whole subtrees of an unpacked application.
- **Wildcards on the command line failed.** `fzip a out.zip src\*` returned
  *"The filename, directory name, or volume label syntax is incorrect (os error
  123)"*. Unix shells expand wildcards before the program sees them; cmd.exe and
  PowerShell do not, so a Windows tool has to do it itself. Fzip now expands
  `*` and `?` in the final path component, and says so plainly when a pattern
  matches nothing.
- **`--overwrite all` could not replace a read-only file**, which is exactly
  what installing over a previous version looks like. The read-only attribute is
  now cleared and the write retried. `--overwrite skip` still leaves such files
  alone, as it should.
- **A file entry colliding with an existing folder** reported a bare permissions
  error. It now says a folder of that name already exists, and never deletes the
  folder to make room.

### Testing

Eight regression tests added, one per defect plus the cases that must keep
working around them. The suite is now 75 integration tests and 14 unit tests.
The archives these tests need are built byte by byte, because .NET's own zip
writer silently rewrites `\` to `/` and would hide the very bug being tested.

## [1.0.0] — 2026-07-25

First public release.

### Added

- **Create ZIP archives** (`fzip a`) with parallel deflate compression, ZIP64 support, and optional AES-256 encryption. Roughly 2× faster than 7-Zip at the same level.
- **Read 7z, tar, gzip, bzip2, xz and zstd** in addition to zip and rar. `.tar.gz`, `.tgz`, `.tar.xz` and friends unwrap the inner tar automatically, so extraction is one step rather than two.
- **ZIP compression methods** beyond deflate: store, bzip2, zstd and xz.
- `fzip t` — verify archive integrity without writing anything to disk.
- `-i` / `-x` glob filters for selecting or excluding entries.
- `--overwrite all|skip|rename|newer` for handling files that already exist.
- `-e` to flatten folder structure on extraction.
- `-V` for version, plus a Windows version resource so publisher and version show in file properties.
- Exit codes matching the 7-Zip convention: `0` ok, `1` warning, `2` error, `7` bad command line.
- Progress bar with transfer rate and ETA.

### Fixed

- **Silent data loss on long paths.** Paths beyond the 260-character `MAX_PATH` limit reported success while the file was never created. The destination is now resolved to the extended `\\?\` form.
- **Silent data loss on skipped entries.** Entries rejected as unsafe were dropped without a word. Each is now reported by name with a reason, and the run exits `1`.
- **Reserved Windows device names.** Entries named `CON`, `NUL`, `LPT1`, `PRN.txt` and similar were written verbatim, producing files that Explorer can barely delete. They are now renamed with a leading underscore.
- **Unbounded memory.** The whole decompressed member was buffered in RAM: a 200 MB member peaked at 233 MB. Members above 32 MB now stream to disk, peaking at 9 MB.
- **Extraction destination.** With no `-o`, output landed in the current working directory. It now lands beside the archive, which is what makes drag-and-drop onto `fzip.exe` behave correctly.
- **Timestamps.** DOS timestamps were treated as UTC when the format defines them as local time, shifting every extracted file by the timezone offset.
- **Duplicate entry names** differing only in letter case (identical on Windows) could be written by two threads at once.

- **Process abort on crafted archives.** 64-bit offsets from the archive were bounds-checked with an addition that wraps in release builds, so a hostile value slipped past the guard and indexed out of range. A 98-byte file was enough to kill the process, in one case mid-extraction. All offset arithmetic is now done in `u64` with saturating addition.
- **Unbounded bzip2, zstd and xz decompression.** The declared entry size was only an allocation hint, never enforced, so a 451-byte archive could expand to hundreds of megabytes of RAM. Decoders are now capped at the declared size plus one byte.
- **ZIP64 boundary off by one.** `0xFFFF` and `0xFFFFFFFF` are the ZIP64 sentinels, so a real value equal to one must be promoted; the comparisons used `>` instead of `>=`. Archiving exactly 65535 entries produced a file fzip could not read back.
- **Encryption defeated streaming.** With `-p`, every file was buffered whole several times over and `--max-memory` was ignored entirely. Encryption now streams: a 150 MB encrypted member peaks at 9.5 MB instead of 158 MB.
- **Local/central ZIP64 mismatch** on streamed entries, **silent corruption** when a write failed mid-archive (the file offset desynchronised and a later entry seeked back into the previous one's data), **entry names over 65535 bytes** truncating the length field, a **race in `--overwrite rename`** where two threads could claim the same name, and `fzip a` **archiving its own output file**.
- Directory entries now carry Unix mode 0755 instead of no permission bits; AES entries declare version-needed 5.1 as the WinZip specification requires.

- **Reduced antivirus false positives.** The executable now carries an application manifest (`asInvoker`, supported-OS list, long-path and UTF-8 declarations) and is built with Control Flow Guard. The manifest also improves behaviour: long paths and non-ASCII names now work through the ANSI Win32 entry points too.
- **RAR support is now an opt-out feature** (`--no-default-features`). RARLAB's UnRAR sources are the whole WinRAR utility, so linking them pulled `AdjustTokenPrivileges`, `OpenProcessToken`, `LookupPrivilegeValueW` and `AllocateAndInitializeSid` into the binary from shutdown and administrator-check code that Fzip never calls. A program that walks directories, encrypts files *and* imports privilege-elevation APIs matches the ransomware fingerprint that machine-learning scanners look for. Building without RAR removes all four imports and 682 KB. See [ANTIVIRUS.md](ANTIVIRUS.md) for the full measurements.

### Security

- Zip-slip paths (`..`, absolute paths, drive letters) are blocked and reported.
- AES entries are authenticated with HMAC-SHA1 **before** decryption, so tampered data is rejected rather than silently decoded into garbage.
- Fzip refuses to *create* ZipCrypto archives; a password always yields AES-256.
- Zip-bomb warning when an archive expands more than 500× to over 1 GB, plus a `--max-memory` ceiling.
- Symbolic and hard links inside tar archives are not extracted.
- `-t` and `--max-memory` reject out-of-range values instead of overflowing.

### Known limitations

- Extracting `.7z` is about 1.4× slower than 7-Zip and `.rar` about 1.3× slower; both gaps are in the decoders themselves and are documented with measurements in the README.
- Creates ZIP only. RAR compression is proprietary and cannot legally be reimplemented.
- No GUI, no shell integration, no self-extracting archives.
