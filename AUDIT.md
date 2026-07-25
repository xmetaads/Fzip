# Fzip — release-readiness audit

Product: **Fzip** · Publisher: **Tcoder LLC** · Version 1.0.0

Audited 2026-07-25 against 7-Zip 25.x and WinRAR 7.x. Method: purpose-built malicious and edge-case archives, plus real process-memory measurement.

> **Scope.** This audit was carried out against the **1.x** implementation, which
> was written in Rust. Fzip 2.0 is a rewrite in Go and shares none of that source.
>
> The audit is kept because its findings were the specification for the rewrite:
> every defect recorded below was carried into 2.0 as a regression test *before*
> the corresponding code was written, and all of those tests pass. The sections
> on formats other than zip are historical only — 2.0 reads zip and nothing else.

## Verdict

**v0.1: not shippable.** Four categories of problem, including two silent data-loss bugs.
**v1.0: shippable**, with the honest limitations listed at the end.

---

## 1. Silent data loss — fixed

| ID | Problem in v0.1 | Evidence | Status |
|---|---|---|---|
| **L1** | Paths over 260 characters: printed `SUCCESS` while **the file was never created** | Zip with a 428-character entry path reported "2 files" extracted; the directory chain stopped at exactly 260 characters and the final file did not exist | **Fixed** — destination is canonicalised to the extended `\\?\` form, so the full depth is created. Test C02 verifies the file exists *and* its contents are correct. |
| **L2** | Skipped entries were never reported | A 4-entry zip (`'   '`, `'...'`, `a/../../evil.txt`, `ok.txt`) produced one file and printed "1 file" with no warning | **Fixed** — every rejected entry prints `SKIPPED (unsafe): <name> (reason)` and the run exits 1. Test C04. |

## 2. Security — fixed

| ID | Problem in v0.1 | Evidence | Status |
|---|---|---|---|
| **S1** | Created files named after Windows devices | `CON`, `PRN.txt`, `NUL`, `COM1`, `LPT1.dat`, `aux` were all written to disk; such files are nearly impossible to delete from Explorer | **Fixed** — renamed to `_CON` etc. Ordinary names like `console.log` are untouched. Test C01 + unit test. |
| **S2** | Held the entire decompressed member in RAM | A 200 MB member drove peak RSS to 232.9 MB; a 10 GB member would have needed 10 GB | **Fixed** — members above 32 MB stream to disk. Same test now peaks at **9.2 MB** (7-Zip: 8.0 MB). Test C05. |
| **S3** | No zip-bomb guard | No ratio check, no memory ceiling | **Fixed** — warns when an archive expands >500× to over 1 GB; `--max-memory` (default 1 GB) caps buffered work. |

Already correct in v0.1 and still verified: zip-slip blocking, CRC checking, AES/ZipCrypto/RAR decryption, ZIP64, multi-volume RAR, magic-byte format detection.

## 3. Feature parity with 7-Zip / WinRAR

| Capability | 7-Zip | WinRAR | fzip v0.1 | fzip v1.0 |
|---|---|---|---|---|
| Create archives | ✅ many | ✅ rar, zip | ❌ | ✅ zip (2× faster than 7-Zip) |
| Extract 7z | ✅ | ✅ | ❌ | ✅ |
| Extract tar/gz/bz2/xz/zst | ✅ | ✅ | ❌ | ✅ (auto-unwraps tar inside .tar.gz) |
| ZIP with bzip2/zstd/xz | ✅ | ✅ | ❌ | ✅ |
| Test command | ✅ | ✅ | ❌ | ✅ `fzip t` |
| Include/exclude filters | ✅ | ✅ | ❌ | ✅ `-i` / `-x` |
| Overwrite modes | ✅ | ✅ | ❌ always overwrote | ✅ all/skip/rename/newer |
| Flatten paths | ✅ | ✅ | ❌ | ✅ `-e` |
| `--version` | ✅ | ✅ | ❌ | ✅ |
| Standard exit codes | ✅ | ✅ | ❌ | ✅ same convention as 7-Zip |
| AES-256 on create | ✅ | ✅ | ❌ | ✅ (refuses to write broken ZipCrypto) |

Still absent by choice: GUI, shell integration, self-extracting archives, and creating 7z/RAR/tar.

## 4. International-release blockers — fixed

| ID | Problem in v0.1 | Status |
|---|---|---|
| **I1** | Entire interface was unaccented Vietnamese (`THANH CONG`, `sai mat khau`) | **Fixed** — interface is English. Console is switched to UTF-8 so non-ASCII *file names* still display correctly. |
| **I2** | No version flag, no metadata in the executable | **Fixed** — `-V`, plus embedded Windows version resource (description, version, copyright visible in file properties). |
| **I3** | No licence; UnRAR attribution missing | **Fixed** — MIT `LICENSE` with the required RARLAB notice. |

## Remaining honest limitations

These are measured, documented in the README, and not worked around:

1. **7z extraction is 1.34× slower than 7-Zip** (4.92 s vs 3.68 s). This is decoder cost, not I/O. A solid 7z archive is a single LZMA2 stream, so per-file parallelism cannot help, and 7-Zip's decoder is hand-optimised assembly.
2. **RAR extraction is 1.29× slower than 7-Zip** (2.08 s vs 1.61 s), though faster than WinRAR itself (2.26 s). fzip uses RARLAB's reference UnRAR, which is single-threaded.
3. **ZIP compression is ~2.6% larger than 7-Zip** at the same level (77.8 MB vs 75.8 MB), in exchange for being 2.34× faster.
4. **Creates ZIP only.**

## Round 2 — line-by-line code audit

The first audit was behavioural: craft an archive, observe what fzip does. The
second read the ZIP writer and reader against the PKWARE APPNOTE field by field.
It found defects that no amount of well-formed-input testing would reach.

### Critical — both confirmed with working proof-of-concept files

| ID | Defect | Evidence | Status |
|---|---|---|---|
| **C1** | Attacker-controlled 64-bit offsets were bounds-checked with `offset + 46 > len`. In release builds that addition wraps, the guard passes, and the slice index goes out of range. With `panic = "abort"` the process dies (`0xC0000409`) instead of reporting an error — and one path dies *inside* the parallel extraction loop, leaving truncated files behind. | A 98-byte crafted archive aborted `fzip l`; an 81-byte one aborted `fzip t`. | **Fixed** — every offset comparison is done in `u64` with `saturating_add`. Tests I01, I02. |
| **C2** | bzip2, zstd and xz entries were decompressed with `read_to_end` and no ceiling. The declared size was used only as an allocation hint, so a hostile archive simply understated it. The zip-bomb warning is computed from the same declared size, so understating it disabled the warning too. | A **451-byte** archive holding 400 MB of bzip2-compressed zeros, declaring 1 KB, drove peak memory to **524 MB**. Scaling is free. | **Fixed** — decoders are wrapped in `Read::take(declared + 1)`; one byte over the declared size aborts the entry. Test I03. |

### High

| ID | Defect | Evidence | Status |
|---|---|---|---|
| **H1** | ZIP64 thresholds used `>` where the spec requires `>=`. `0xFFFF` and `0xFFFFFFFF` *are* the sentinels, so a real value equal to one must move into a ZIP64 record. | Archiving 65534 files produced an end record with `total entries = 0xFFFF` and **no** ZIP64 locator; `fzip l` then failed to read fzip's own archive. | **Fixed** — all six comparisons now use `>=`. Test I04. |
| **H2** | `-p` disabled the streaming path entirely, so every encrypted file was buffered whole, three or four times over (raw, compressed, encrypted, output), multiplied by thread count. `--max-memory` was not consulted at all. A 20 GB file would have tried to allocate 60–80 GB and aborted. | — | **Fixed** — a streaming AES-256 writer keeps the CTR keystream and HMAC across chunks, so encryption now streams like everything else. Budgeting also multiplies by the real per-file cost. A 150 MB encrypted member peaks at **9.5 MB**, down from 158 MB. Test I05. |
| **H3** | DOS timestamps were converted with one cached "right now" UTC offset. DOS time is local time with per-date DST rules, so any file from the other DST phase was off by an hour and drifted further on every round trip. | — | **Fixed** — conversion goes through `SystemTimeToTzSpecificLocalTime`, which applies the rules in force *at that timestamp*. Unit tests cover summer and winter moments. |

### Medium

| ID | Defect | Status |
|---|---|---|
| **M1** | A streamed entry wrote a ZIP64 local header but a non-ZIP64 central record, so the two disagreed for every file over the streaming threshold. | **Fixed** — the central record inherits the local header's ZIP64 decision. |
| **M2** | A failed write left bytes in the file without advancing `offset`, so a later streamed entry seeked backwards *into the previous entry's payload* and overwrote 24 bytes of it. Silent corruption on top of a reported error, and the half-written archive was left on disk. | **Fixed** — any write failure aborts the run and removes the incomplete archive. |
| **M3** | Entry names over 65535 bytes wrapped the 16-bit length field, desynchronising every reader. | **Fixed** — over-long names are skipped with a warning. |
| **M4** | `--overwrite rename` chose a free name with `exists()` then created it, so two threads could pick the same candidate and one file was lost. | **Fixed** — the name is claimed atomically with `create_new`. |
| **M5** | `fzip a backup.zip .` would walk the output archive into its own input and read a file that was concurrently growing. | **Fixed** — the output path is excluded from the input walk. |
| **M6** | A tiny archive declaring a huge entry count reserved ~90 MB up front. | **Fixed** — the reservation is capped by what the file size can actually hold. |

### Low

All fixed: AES entries now declare version-needed 5.1 as the WinZip spec requires; directory entries carry mode 0755 instead of no permission bits at all; `dos_to_unix` rejects nonsense hour/minute/second fields rather than inventing "hour 31"; and pre-1980 timestamps clamp to 1980-01-01 rather than keeping a month and day that no longer belong to the year.

### Verified correct, no change needed

AES extra-field byte patching and compressed-size accounting (the 28 bytes of salt, verifier and MAC are included); encrypt-then-MAC ordering; AE-2 CRC handling on both sides; the seek-back arithmetic in the streaming writer; rayon usage in the compressor; and the EOCD64 and locator record layouts.

## Test coverage

67 integration tests and 14 unit tests, all passing, with `cargo clippy` clean. Integration tests compare MD5 of every extracted file against the source rather than only checking exit status. Every defect above has a dedicated regression test, including the two crafted archives that used to abort the process and the 451-byte bomb that used to consume half a gigabyte.
