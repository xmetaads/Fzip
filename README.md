# Fzip

**Published by Tcoder LLC.** MIT licensed. Reviewed and allow-listed by Microsoft.

A single-file, no-install zip tool for Windows. Portable in the spirit of
Microsoft's `azcopy.exe` — one self-contained `.exe` you copy anywhere and drive
from the command line. (That comparison is about how the tool is *shipped*, not
how it is built: azcopy is written in Go, Fzip in Rust.)

**Reads** zip · **Writes** zip, optionally encrypted with AES-256

```bash
fzip x archive.zip
```

---

## Why it exists

Extracting a ZIP on Windows is slower than it needs to be. 7-Zip, WinRAR and
PowerShell's `Expand-Archive` decompress ZIP entries **one at a time on a single
core**. Fzip decompresses every entry **in parallel**, one per CPU core, straight
out of a memory-mapped file.

## Benchmarks

AMD Ryzen AI 9 465 (20 threads), Windows 11. 303 files / 183.3 MB (77.8 MB
compressed, including 120 MB of barely-compressible binary). Best of 3 runs.
Reproduce with `.\run_bench.ps1`.

**Extracting ZIP** — the main use case:

| Tool | Time | Relative |
|---|---|---|
| **fzip** | **0.668 s** (274 MB/s) | fastest |
| tar.exe (built into Windows 11) | 0.821 s | 1.23× slower |
| WinRAR | 1.670 s | 2.50× slower |
| 7-Zip | 1.794 s | 2.69× slower |
| .NET `ZipFile` | 2.647 s | 3.96× slower |

**Creating ZIP** (default level):

| Tool | Time | Output |
|---|---|---|
| **fzip -mx5** | **1.526 s** | 77.8 MB |
| 7-Zip -mx5 | 3.413 s (2.24× slower) | 75.8 MB |
| .NET `ZipFile` | 17.423 s (11.4× slower) | 77.8 MB |

7-Zip's deflate encoder squeezes out about 2.6% more; Fzip trades that for
roughly double the speed. Use `-mx9` when size matters more.

**Verifying only**, which isolates decompression from the disk:

| Tool | Time | Throughput |
|---|---|---|
| **fzip t** | **0.314 s** | 584 MB/s |
| 7-Zip t | 1.403 s (4.47× slower) | 131 MB/s |

**Peak RAM** on a single 200 MB member: fzip 7.5 MB, 7-Zip 8.0 MB. Fzip streams
entries above 32 MB to disk instead of buffering them, so this figure does not
grow with the archive.

Throughput depends on what is in the archive. The one above is deliberately
hostile — two thirds is near-random binary that will not compress, so storage
dominates. On an archive of ordinary documents the same binary verifies far
faster; the worker-count table in [web/usage.md](web/usage.md) uses one of those
and reaches a different number. Both are real. Neither is *the* number.

## What changed in 1.2 and 1.3

Fzip reads **zip only**. Version 1.0 also read rar, 7z, tar, gz, bz2, xz and zst;
1.2 dropped all of them. Hand Fzip one of those and it names the format rather
than reporting a broken zip:

```
> fzip x archive.rar
fzip: archive.rar is a RAR archive, and this version reads zip only
```

If you need those formats, keep a tool that has them. See
[CHANGELOG.md](CHANGELOG.md) for what 1.3 fixed — including a zip bomb that
could reach the disk before being caught.

## Install

Download: **<https://fzip.org/download/fzip.exe>**

There is no installer. Download `fzip.exe` and run it. It needs no runtime, no
DLLs and no registry entries — the C runtime is linked into the executable, so
there is no Visual C++ Redistributable to install first.

Check what you downloaded:

```powershell
Get-FileHash fzip.exe -Algorithm SHA256
# 3FB3B422A400C8DF95904B488DCB7B4277D04E757BE9D6EA4D0A261DC2CA7A8C
```

That is the exact build Microsoft analysed and allow-listed. Fzip is **not
code-signed** — the file is vouched for by that review, the published hash and
the readable source, not by a certificate. Saying so is deliberate: claiming a
signature that does not exist would be worse than claiming nothing.

> **Antivirus history.** Earlier releases drew `Trojan:Win32/Wacatac.B!ml` from
> Microsoft Defender. It was a machine-learning false positive, submitted to
> Microsoft and corrected. [ANTIVIRUS.md](ANTIVIRUS.md) records what was
> measured rather than assumed: the verdict followed a **file hash**, not the
> code, not the implementation language (a minimal Rust binary and a minimal Go
> binary both scanned clean), and not any single dependency. It is kept because
> the same thing can happen to any small, unsigned, low-prevalence tool.

## Usage

```
fzip x <archive.zip> [options]    extract
fzip <archive.zip>                extract (shorthand, also works by drag-and-drop)
fzip l <archive.zip>              list contents
fzip t <archive.zip>              test integrity, write nothing
fzip a <archive.zip> <files...>   create a zip
```

| Option | Meaning |
|---|---|
| `-o <dir>` | output folder (default: the archive name, beside the archive) |
| `-p <pass>` | password; prompts securely if the value is omitted |
| `-t <n>` | worker threads (default: every core) |
| `-i <glob>` | include only matching names |
| `-x <glob>` | exclude matching names |
| `-e` | flatten: ignore folder structure |
| `-y` | assume yes (overwrite the archive when creating) |
| `--overwrite <m>` | `all` \| `skip` \| `rename` \| `newer` (default `all`) |
| `-mx<0-9>` | compression level for `a` (0 = store, 9 = best) |
| `--max-memory <n>` | RAM cap, e.g. `512M` or `2G` (default 1G) |
| `--no-crc` | skip CRC verification |
| `--progress` | force the progress bar even when redirected |
| `--no-pause` | never wait for a keypress (installers, scripts) |
| `-q` / `-v` | quiet / verbose |

Exit codes: `0` ok, `1` warning, `2` error, `7` bad command line.

Full reference: [web/usage.md](web/usage.md), also served at
<https://fzip.org/usage.md>.

## Building

Requires Rust 1.85 or newer, MSVC toolchain.

```bash
cargo build --release
```

`.cargo/config.toml` turns on Control Flow Guard and links the C runtime
statically. Both matter for what ships — see the comments in that file.

Windows only. The console handling, path rules and memory mapping are written
directly against the Win32 API, so there is no portable build.

## Testing

```powershell
cargo test --release   # 15 unit tests
cargo clippy --release --all-targets
.\run_tests.ps1        # 67 integration tests
.\run_bench.ps1        # comparative benchmark
```

## Safety

Fzip opens files it did not create, so the hostile cases are tested rather than
assumed: path traversal, reserved Windows device names, paths past 260
characters, zip bombs on both the buffered and streaming paths, corrupt central
directories, and offsets crafted to index out of range. Each has a regression
test in `run_tests.ps1`, sections C and I.

Encrypted entries are authenticated **before** decryption, so tampered data is
rejected rather than decoded into garbage.

### What is in the binary

Checked before release, and worth re-checking on every one:

| Check | Result |
|---|---|
| Control Flow Guard | present |
| ASLR, DEP, high-entropy address space | present |
| Privilege APIs imported | none |
| Process injection, registry writing, network APIs | none |
| Imported DLLs | `kernel32`, `ntdll`, `user32`, `bcryptprimitives`, `dbghelp` |
| `.text` entropy | 6.33 — ordinary compiled code, not packed |
| Dependency graph | 78 crates, down from 130 in 1.0.2 |

One import worth naming before anyone else does: **`IsDebuggerPresent`**. It is
not in Fzip's source — it arrives with the statically linked MSVC C runtime,
which calls it on the abort path. Linking the runtime dynamically would move it
into `VCRUNTIME140.dll` rather than remove the call, at the cost of requiring the
Visual C++ Redistributable. For scale: Windows' own `tar.exe` and WinRAR's
`Rar.exe` import it too.

## Layout

```
src/main.rs         entry point, format detection, the double-click pause
src/cli.rs          argument parsing, glob filters, help
src/common.rs       exit codes, formatting, DOS timestamps, progress bar
src/safepath.rs     zip-slip defence, device names, long paths
src/crypto.rs       WinZip AES-256/192/128 and legacy ZipCrypto
src/zipread.rs      parsing, decryption, parallel decompression
src/zipwrite.rs     parallel compression, ZIP64, AES-256
build.rs            icon, manifest and version resource
web/                the fzip.org site — see WEBSITE.md
brand/              logo and icon sources
```
