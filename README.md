# Fzip

**Published by Tcoder LLC.** MIT licensed.

A single-file, no-install zip tool for Windows. Portable in the spirit of
Microsoft's `azcopy.exe` — one self-contained `.exe` you copy anywhere and drive
from the command line.

**Reads** zip · **Writes** zip, optionally encrypted with AES-256

```bash
fzip x archive.zip
```

Written in Go, with **no third-party dependencies** — the only code in the binary
is this repository and the Go standard library.

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

**Extracting ZIP:**

| Tool | Time | Relative |
|---|---|---|
| tar.exe (built into Windows 11) | **0.831 s** | fastest |
| **fzip** | 0.846 s (217 MB/s) | 1.02× slower |
| 7-Zip | 1.814 s | 2.18× slower |
| WinRAR | 1.861 s | 2.24× slower |
| .NET `ZipFile` | 2.644 s | 3.18× slower |

**Creating ZIP** (default level):

| Tool | Time | Output |
|---|---|---|
| **fzip -mx5** | **1.328 s** | 74.3 MB |
| 7-Zip -mx5 | 3.544 s (2.67× slower) | 75.8 MB |
| .NET `ZipFile` | 17.580 s (13.2× slower) | 77.8 MB |

**Verifying only**, which isolates decompression from the disk:

| Tool | Time | Throughput |
|---|---|---|
| **fzip t** | **0.507 s** | 362 MB/s |
| 7-Zip t | 1.413 s (2.79× slower) | 130 MB/s |

**Peak RAM** on a single 200 MB member: fzip 6.9 MB, 7-Zip 8.0 MB.

### Read these numbers honestly

- **Extraction is a tie with `tar.exe`**, not a win. Windows ships a competent
  ZIP extractor and on this archive it is 2% ahead. The margin over 7-Zip and
  WinRAR is real — roughly 2.2× — but "fastest on Windows" would be false, so it
  is not claimed.
- **Throughput depends on how compressible the archive is.** The archive above
  is deliberately hostile: two thirds of it is near-random binary that will not
  compress, so storage dominates. On an archive of ordinary documents the same
  binary verifies at 1,304 MB/s. Both numbers are real; neither is *the* number.
- **Fzip 1.x extracted faster.** That release was written in Rust and used
  libdeflate, which decompresses faster than Go's `compress/flate`. The move to
  Go cost roughly 25% on the extract path. It bought a binary with no
  third-party code in it; that trade was made deliberately, not accidentally.

## What changed in 2.0

Fzip 2.0 is a rewrite in Go, and it reads **zip only**. Version 1.x also read
rar, 7z, tar, gz, bz2, xz and zst; all of that is gone. Hand Fzip one of those
and it names the format rather than reporting a broken zip:

```
> fzip x archive.rar
fzip: archive.rar is a RAR archive, and this version reads zip only
```

If you need those formats, keep a tool that has them. Details in
[CHANGELOG.md](CHANGELOG.md).

## Install

Download: **<https://fzip.org/download/fzip.exe>**

There is no installer. Download `fzip.exe` and run it. It needs no runtime, no
DLLs and no registry entries.

Verify what you downloaded:

```powershell
Get-FileHash fzip.exe -Algorithm SHA256
# PLACEHOLDER_SHA256
```

Go also records the source inside the executable, which is a stronger check than
the hash alone:

```powershell
go version -m fzip.exe
```

> **Seeing an antivirus warning such as `Wacatac.B!ml`?** It is a
> machine-learning false positive. [ANTIVIRUS.md](ANTIVIRUS.md) documents what
> was measured rather than assumed: the implementation language is not the cause
> (a minimal Rust binary and a minimal Go binary both scan clean), no single
> dependency was the cause, and the same source recompiled can flip the verdict.
> It attaches to a **file hash**, driven by the absence of a code signature and
> zero download history. A code-signing certificate is the only real fix.

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
| `-o <dir>` | output folder (default: the archive name) |
| `-p <pass>` | password; prompts securely if omitted |
| `-t <n>` | worker count (default: every core) |
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

Requires Go 1.24 or newer. There is nothing to fetch — the module has no
dependencies.

```bash
go build -trimpath -ldflags "-s -w" -o fzip.exe .
```

The icon, manifest and version resource come from `resource.syso`, which is
committed so an ordinary build needs no extra tooling. Regenerate it only when
`versioninfo.json`, `fzip.manifest` or `brand/fzip.ico` change:

```bash
go run github.com/josephspurrier/goversioninfo/cmd/goversioninfo@v1.7.0 -o resource.syso versioninfo.json
```

Windows only. The console handling, path rules and memory mapping are written
directly against the Win32 API, so there is no portable build.

## Testing

```powershell
go test ./...          # 18 unit tests
.\run_tests.ps1        # 66 integration tests
.\run_bench.ps1        # comparative benchmark
```

`go vet ./...` reports one diagnostic, on the memory-mapping call: turning the
address the OS hands back into a slice needs a `uintptr` → `unsafe.Pointer`
conversion that vet cannot verify. It is checked at runtime instead, and
`go test -gcflags=all=-d=checkptr=1 ./...` passes. The reasoning is written out
in [winapi_windows.go](winapi_windows.go).

## Safety

Fzip opens files it did not create, so the hostile cases are tested rather than
assumed: path traversal, reserved Windows device names, paths past 260
characters, zip bombs, corrupt central directories, and offsets crafted to index
out of range. Each has a regression test in `run_tests.ps1`, sections C and I.

Encrypted entries are authenticated **before** decryption, so tampered data is
rejected rather than decoded into garbage.

## Layout

```
main.go             entry point, format detection, the double-click pause
cli.go              argument parsing, glob filters, help
common.go           exit codes, formatting, DOS timestamps, progress bar
safepath.go         zip-slip defence, device names, long paths
crypto.go           WinZip AES-256/192/128 and legacy ZipCrypto
zipread.go          parsing, decryption, parallel decompression
zipwrite.go         parallel compression, ZIP64, AES-256
winapi_windows.go   console, password prompt, memory mapping
fzip_test.go        unit tests
web/                the fzip.org site — see WEBSITE.md
brand/              logo and icon sources
```
