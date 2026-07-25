# Fzip

**Published by Tcoder LLC.** MIT licensed.

A single-file, no-install archive tool for Windows. Portable in the spirit of Microsoft's `azcopy.exe` — one self-contained `.exe` you copy anywhere and drive from the command line. (That comparison is about how the tool is *shipped*, not how it is built: azcopy is written in Go, Fzip in Rust.)

**Reads** zip · rar · 7z · tar · gz · bz2 · xz · zst
**Writes** zip, optionally encrypted with AES-256

```bash
fzip x archive.zip
```

---

## Why it exists

Extracting a ZIP on Windows is slower than it needs to be. 7-Zip, WinRAR, and PowerShell's `Expand-Archive` all decompress ZIP entries **one at a time on a single core**. fzip decompresses every entry **in parallel**, one per CPU core, straight out of a memory-mapped file.

On a 20-thread machine that makes ZIP extraction about **2.7× faster than 7-Zip and WinRAR**.

## Benchmarks

AMD Ryzen AI 9 465 (20 threads), Windows 11. 303 files / 183 MB (78 MB compressed). Best of 3 runs. Reproduce with `.\run_bench.ps1`.

**Extracting ZIP** — fzip's main use case:

| Tool | Time | Relative |
|---|---|---|
| **fzip** | **0.67 s** (274 MB/s) | fastest |
| tar.exe (built into Windows 11) | 0.83 s | 1.24× slower |
| WinRAR | 1.78 s | 2.67× slower |
| 7-Zip | 1.81 s | 2.71× slower |
| .NET `ZipFile` | 2.55 s | 3.82× slower |

**Creating ZIP** (default level):

| Tool | Time | Output size |
|---|---|---|
| **fzip -mx5** | **1.65 s** | 77.8 MB |
| 7-Zip -mx5 | 3.87 s (2.34× slower) | 75.8 MB |
| .NET `ZipFile` | 17.45 s (10.6× slower) | 77.8 MB |

7-Zip's deflate encoder squeezes out ~2.6% more; fzip trades that for roughly double the speed.

**Where fzip is *not* the fastest** — measured, not hidden:

| Task | Winner | fzip |
|---|---|---|
| Extract 7z | 7-Zip 3.68 s | 4.92 s (1.34× slower) |
| Extract RAR | 7-Zip 1.61 s | 2.08 s (1.29× slower, still ahead of WinRAR's 2.26 s) |

Both gaps are in the decoder itself. 7-Zip's LZMA2 and RAR decoders are hand-tuned assembly refined over two decades; fzip uses portable Rust implementations, and a solid 7z archive is a single stream that cannot be split across cores. If your workload is mostly `.7z`, use 7-Zip.

**Peak memory**, extracting one 200 MB member: fzip 9.2 MB, 7-Zip 8.0 MB. fzip streams large entries to disk rather than buffering them.

## Install

There is no installer. Download `fzip.exe` and run it. It needs no runtime, no DLLs, and no registry entries.

Verify what you downloaded:

```powershell
Get-FileHash fzip.exe -Algorithm SHA256
# 02400B918E2C0F974ADE4E45839441FCD5B3B3CF48D05862ADA7EBC6169F8BEA
```

That hash identifies the published binary. Building from source yourself yields a functionally identical executable with a different hash, because Rust builds are not bit-for-bit reproducible.

> **Seeing an antivirus warning such as `Wacatac.B!ml`?** It is a
> machine-learning false positive. [ANTIVIRUS.md](ANTIVIRUS.md) documents what
> was actually measured: the language is not the cause (a minimal Rust binary
> and a minimal Go binary both scan clean), no single dependency is the cause,
> and the same source recompiled can flip the verdict — because it attaches to a
> file hash, driven by the absence of a code signature and zero prevalence.
>
> One genuine contributor *was* found and fixed: RAR support dragged in four
> privilege APIs (`AdjustTokenPrivileges` and friends) that Fzip never calls,
> from UnRAR's unused shutdown and admin-check code. RAR is now an opt-out
> feature:
>
> ```bash
> cargo build --release --no-default-features   # no RAR, none of those imports, 682 KB smaller
> ```

## Usage

```
fzip x <archive> [options]        extract
fzip <archive>                    extract (shorthand, also works by drag-and-drop)
fzip l <archive>                  list contents
fzip t <archive>                  test integrity, write nothing
fzip a <archive.zip> <files...>   create a zip
```

| Option | Meaning |
|---|---|
| `-o <dir>` | output folder (default: the archive name) |
| `-p <pass>` | password; prompts securely if omitted |
| `-t <n>` | CPU threads (default: all) |
| `-i <glob>` | include only matching names |
| `-x <glob>` | exclude matching names |
| `-e` | flatten: ignore folder structure |
| `-y` | assume yes (overwrite the archive when creating) |
| `--overwrite <mode>` | `all` (default), `skip`, `rename`, `newer` |
| `-mx<0-9>` | compression level for `a` (0 = store, 9 = best) |
| `--max-memory <n>` | RAM cap, e.g. `512M`, `2G` (default 1G) |
| `--no-crc` | skip CRC verification |
| `--progress` | force the progress bar even when redirected |
| `-q` / `-v` | quiet / verbose |
| `-V` | version |

Examples:

```bash
fzip x data.zip -o D:\out
fzip x secret.7z -p MyPass123
fzip x big.rar -x "*.tmp" --overwrite skip
fzip a backup.zip photos docs -mx9 -p MyPass123
fzip t archive.zip
```

Format is detected from the file's **magic bytes**, not its extension — a `.zip` that is really a RAR still extracts correctly.

### Awkward archives

Not every producer writes well-formed names. Windows' own `tar.exe` stores non-ASCII names without the UTF-8 flag, substituting `?` for characters it cannot encode. `Expand-Archive` then aborts the whole archive with *"Illegal characters in path"*; fzip sanitises the offending characters and extracts the data anyway. (On a normal UTF-8 zip, such as one written by .NET or 7-Zip, `Expand-Archive` handles non-ASCII names fine — the problem is the producer, not the alphabet.)

### Progress

```
Extracting test.zip (303 files, 183.30 MB) -> D:\out
[==========>             ]  42.1%  128/303  77.12 MB  268.40 MB/s  ETA 1s
[========================] 100.0%  303/303  183.30 MB  269.10 MB/s  ETA --
Done: 303 files, 77.84 MB -> 183.30 MB in 0.682s (268.75 MB/s)
```

### Exit codes

Same convention as 7-Zip, so existing scripts keep working: `0` success · `1` warning (something was skipped) · `2` error · `7` bad command line.

## Encryption

**Reading:** AES-256/192/128 (WinZip AE-1 and AE-2), legacy ZipCrypto, RAR data and header encryption (`rar a -p` and `-hp`), and encrypted 7z.

**Writing:** AES-256 only. fzip deliberately will not create ZipCrypto archives — that cipher is broken and recoverable with a known-plaintext attack. If you ask fzip for a password, you get real encryption.

AES entries are verified with HMAC-SHA1 **before** decryption (encrypt-then-MAC), so tampered data is rejected instead of silently producing garbage. Salts come from the OS CSPRNG and are fresh on every entry.

Archives fzip encrypts are ordinary WinZip-AES archives — 7-Zip and WinRAR open them normally (verified in the test suite).

## Safety

Every item below is covered by an automated test:

- **Zip-slip blocked.** Entries containing `..`, absolute paths, or drive letters cannot write outside the destination, and each blocked entry is *reported* rather than silently dropped.
- **Reserved device names renamed.** An entry called `CON`, `NUL`, `LPT1`, or `PRN.txt` becomes `_CON` and so on. Files actually named after a device are nearly impossible to delete from Explorer.
- **Long paths work.** Paths beyond the 260-character `MAX_PATH` limit are written through the extended `\\?\` form. fzip never reports success for a file it failed to create.
- **Bounded memory.** Entries above 32 MB stream to disk instead of being buffered, so a 50 GB member does not need 50 GB of RAM. `--max-memory` caps the rest.
- **Zip-bomb warning** when an archive expands more than 500× to over 1 GB.
- **CRC verified** on every entry by default.
- **Symlinks in tar are not extracted** — on Windows they are an escape vector.
- **Duplicate entry names** (including case-only differences, which Windows treats as identical) are resolved before extraction so two threads never write the same file.

## Testing

```bash
.\run_tests.ps1     # 59 integration tests
cargo test          # 9 unit tests
.\run_bench.ps1     # benchmarks
```

The integration suite compares **MD5 of every extracted file** against the source — not merely "the command exited 0". It covers archives produced by .NET, 7-Zip, and WinRAR; all four encryption schemes; corrupted archives; ZIP64 with 70 000 entries; multi-volume RAR; solid RAR; header-encrypted RAR; every command-line option; and each security property listed above.

## Limitations

- **Creates ZIP only.** No 7z, RAR, or tar creation. RAR compression is proprietary and cannot legally be reimplemented.
- **Slower than 7-Zip on 7z and RAR** — see the benchmark table.
- **No GUI**, no shell integration, no self-extracting archives.
- **ZIP methods supported:** store, deflate, bzip2, zstd, xz. LZMA-in-zip and Deflate64 are reported as unsupported rather than mis-extracted.

## Build

```bash
cargo build --release
```

Produces `target\release\fzip.exe`, a single self-contained binary (~2.8 MB; the UnRAR and LZMA decoders are statically linked).

## License

Copyright (c) 2026 **Tcoder LLC**. MIT licensed — see [LICENSE](LICENSE).

RAR support uses RARLAB's official UnRAR sources, which may not be used to create a RAR-compatible *archiver*; Fzip only decompresses RAR. Other dependencies are MIT and/or Apache-2.0.

Day-to-day usage instructions: [GUIDE.md](GUIDE.md).
