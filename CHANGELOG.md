# Changelog

All notable changes to **Fzip**, published by Tcoder LLC.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [Semantic Versioning](https://semver.org/).

## [1.3.0] - 2026-07-25

Back to **Rust**, still **zip only**. The work went into what anyone vouching for
this binary would have to stand behind: nothing unnecessary linked in, nothing
imported that is never called, and no path where hostile input reaches the disk
before it is checked.

The version number returns to the 1.x line because that is where the Rust
implementation left off. 2.0.0 was the Go build; it is superseded.

**Microsoft has analysed this build and allow-listed it.** The
`Trojan:Win32/Wacatac.B!ml` detection that affected 1.0.1 is corrected, and
SmartScreen passes the file. The binary is not code-signed; it is vouched for by
that review, the published SHA-256, and source anyone can rebuild.

### Fixed

- **A zip bomb could write to disk unchecked.** Entries above 32 MB stream
  rather than buffer, and until now that path had **no size cap at all** —
  only the buffered path checked. An entry declaring 40 MB and expanding to
  200 MB was written to disk in full and reported afterwards, which is too late
  to matter. The bzip2, zstd and xz paths had the cap; DEFLATE, the method
  almost every real zip uses, did not.

  The limit now sits in the one place every byte passes through, and is checked
  **before** the write, so oversized bytes never reach the filesystem. Regression
  test `I03b` confirms exactly 40 MB — the declared size — lands on disk instead
  of 200 MB.
- **The zip-bomb refusal read as decoder jargon.** A caught bomb reported
  *"Output limit exceeded, set limit was 1088 and output size is 1292"*. Both
  paths now say the same plain thing: the entry expands beyond its declared size.

### Changed

- **The C runtime is now linked statically** (`-C target-feature=+crt-static`).
  Fzip previously imported `VCRUNTIME140.dll`, which is part of the Visual C++
  Redistributable and **not** part of a clean Windows install — so the product's
  central promise, that you can copy one .exe anywhere and run it, was false on
  exactly the machines least likely to have developer tooling. The import table
  is now nothing but genuine Windows system DLLs. Costs 98 KB.
- **The version resource no longer claims to include UnRAR.** It did until 1.1;
  the string outlived the code.
- **The dependency graph went from 130 crates to 78** — thirteen direct
  dependencies removed along with the formats they served, and 52 crates fewer
  once their transitive dependencies go too. Among them RARLAB's UnRAR C++
  sources, which are the whole WinRAR utility rather than just an extractor, and
  which brought `AdjustTokenPrivileges`, `OpenProcessToken`,
  `LookupPrivilegeValueW` and `AllocateAndInitializeSid` into the import table
  from shutdown and administrator-check code Fzip never called. Verified absent:
  the binary now imports no privilege API at all.
- **1.54 MB, down from 2.87 MB** in 1.0.2.

### Verified before release

| Check | Result |
|---|---|
| Control Flow Guard | present |
| ASLR, DEP, high-entropy address space | present |
| Privilege APIs in the import table | none |
| Process injection, registry writing, network APIs | none |
| Imported DLLs | `kernel32`, `ntdll`, `user32`, `bcryptprimitives`, `dbghelp` only |
| `.text` entropy | 6.33 — ordinary compiled code, not packed |
| Defender scan, with and without Mark-of-the-Web | clean |
| Tests | 67 integration, 15 unit, none skipped; clippy clean |

One import is worth naming rather than leaving to be found: `IsDebuggerPresent`
appears in the table. It is not in Fzip's source — the statically linked MSVC C
runtime calls it on its abort path. Linking dynamically would hide it inside
`VCRUNTIME140.dll` rather than remove it, and would reintroduce the redistributable
dependency this release just removed. Windows' own `tar.exe` and WinRAR's
`Rar.exe` import it as well.

### Note on the antivirus question

The measurements stand: the `Wacatac.B!ml` verdict followed a **file hash**, not
the language, not a dependency, and not Mark-of-the-Web. See
[ANTIVIRUS.md](ANTIVIRUS.md). What settled it was submitting the file to
Microsoft for analysis — and what made that submission easy to accept was having
nothing left in the binary for an analyst to hesitate over: no privilege APIs,
no third-party C++, five imported DLLs, and 1/69 on VirusTotal with Microsoft as
the only engine flagging it.

The allow-list entry covers **this hash**. Any future release starts again from
zero prevalence, so submit each one before announcing it.

## [2.0.0] - 2026-07-25

Fzip is now written in **Go**, and reads **zip only**. Both are breaking changes,
which is what the major version number is for. Behaviour, command line, exit
codes and archive output are otherwise unchanged: every 1.x command still means
the same thing.

### Removed

- **Rust.** The entire implementation was rewritten in Go. Nothing of the
  previous source remains.
- **Every format except zip.** Version 1.x read rar, 7z, tar, gz, bz2, xz and
  zst. None of those are read any more. Handed one, Fzip identifies it by name
  and stops, so the failure explains itself:

  ```
  > fzip x archive.rar
  fzip: archive.rar is a RAR archive, and this version reads zip only
  ```

- **BZip2, LZMA, Zstd and XZ used as a compression method inside a zip.**
  Deflate and stored remain, which covers essentially every zip in circulation.
- **All third-party dependencies.** The binary now contains only this repository
  and the Go standard library. WinZip AES uses `crypto/aes`, `crypto/hmac` and
  the standard `crypto/pbkdf2`; deflate uses `compress/flate`; CRC-32 uses
  `hash/crc32`. Nothing is fetched at build time.

### Changed

- **Extraction is about 25% slower than 1.x.** Measured, not estimated: the same
  183.3 MB benchmark archive extracts in 0.846 s against 0.67 s for the Rust
  build, because `compress/flate` decompresses more slowly than libdeflate. That
  still leaves Fzip roughly 2.2× ahead of 7-Zip and WinRAR, and 2.7× ahead of
  7-Zip at creating archives, but it is a real regression and is stated here
  rather than buried.
- **`tar.exe`, built into Windows 11, now edges out extraction** on that archive
  by 2%. The README no longer claims Fzip is the fastest ZIP extractor on
  Windows, because on this measurement it is not.
- **Control Flow Guard is gone**, because the Go toolchain does not emit it.
  ASLR, DEP and high-entropy address space are still present, and the binary is
  still unpacked and unobfuscated.
- **Timestamp conversion is simpler and no less correct.** Go's `time.Local`
  applies the timezone rules in force *on the date being converted*, so the
  hand-written DST handling that 1.x needed against the Win32 API is gone while
  summer and winter timestamps still round-trip exactly.
- Binary size is 2.5 MB, down from 2.8 MB.

### Fixed

- **A file entry colliding with an existing folder** reported a bare "is a
  directory" error instead of the intended message. The 1.x code recognised this
  case only when the OS reported a *permission* error, which Windows does not do
  here, so the explanation never appeared. The check no longer depends on which
  errno the platform chooses.

### Testing

66 integration tests and 18 unit tests, none skipped. Every defect found in the
1.x audits and in the field came across as a regression test first. Two new
tests cover ground 1.x did not: `J03b` proves the double-click pause actually
happens when it should, so that `J03` is testing something, and `J03c` covers
`FZIP_NO_PAUSE`.

## [1.0.2] - 2026-07-25

A rebuild that retires the 1.0.1 binary, whose SHA-256 picked up a false
malicious verdict in Microsoft Defender's cloud. **No behaviour changed.** If
1.0.1 runs for you, nothing here fixes a bug you have.

### Why this release exists

The 1.0.1 download was blocked as `Trojan:Win32/Wacatac.B!ml`, ThreatID
2147735505. Measured on Security Intelligence 1.455.339.0, engine
1.1.26060.3008, with cloud protection on Advanced and Block at First Sight
enabled:

| Test | Result |
|---|---|
| The published 1.0.1 file, downloaded twice, 20 minutes apart | blocked both times |
| The same file before any Mark-of-the-Web was applied | blocked |
| The same source rebuilt locally | clean |
| A local build with Mark-of-the-Web applied | clean |
| Three builds **with** RAR support, different hashes | clean, 3/3 |
| Three builds **without** RAR support | clean, 3/3 |

One local build was detected at 19:51 and scanned clean at 20:11 - same bytes,
same machine, twenty minutes apart. The verdict therefore attaches to a
**specific file hash**. It is not the code, not Mark-of-the-Web, and not the
statically linked UnRAR sources that had been the leading suspect. Files built
shortly after a detection were briefly condemned as well and then cleared,
consistent with a cloud verdict spreading across similar hashes and expiring.

Publishing a new build gives the download a hash with no verdict against it.
That is a workaround, not a cure: every release starts at zero prevalence, and
an unsigned executable can be condemned again. A code-signing certificate for
Tcoder LLC is the only durable fix. The 1.0.1 hash has been submitted to
Microsoft for correction.

### Changed

- `Cargo.toml` declared `tcoder.dev/fzip` as the homepage and `tcoder-llc/fzip`
  as the repository. Neither exists. They are now `fzip.org` and
  `github.com/xmetaads/Fzip`.
- The version resource embedded in `fzip.exe` now names the homepage and the
  source repository, so anyone inspecting an unsigned binary can check its
  origin against something public instead of taking the publisher field on
  trust.

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
