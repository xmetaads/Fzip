# Why antivirus software flags Fzip, and what was measured

Users report `Trojan:Win32/Wacatac.B!ml` and `Trojan:Win32/Wacapew.C!ml` from
Microsoft Defender. This document reports what was actually tested, not what
sounds plausible.

## Summary of findings

| Hypothesis | Verdict | Evidence |
|---|---|---|
| "It's because Fzip is written in Rust" | **Ruled out** | A minimal Rust `hello world` scans clean. So does a minimal Go one. The language is not the trigger. |
| "One of the dependencies is dirty" | **Ruled out** | UnRAR, sevenz-rust2, the AES/HMAC/PBKDF2 stack, the bzip2/xz/zstd C codecs and the deflate codecs were each linked into a separate test binary. All clean. All of them linked together into one 928 KB binary: also clean. |
| "The verdict is stable for a given source" | **False** | One Fzip build was quarantined; rebuilding the *same source* produced a binary that scans clean. The verdict attaches to a file hash, not to the code. |
| "Fzip imports privileged APIs it never uses" | **Confirmed, and fixed** | See below. |

## How the detection actually reached the user

The Defender record shows the file was flagged on **download**, not on local
execution:

```
ThreatID 2147735505  =  Trojan:Win32/Wacatac.B!ml
file:_C:\Users\admin\Downloads\fzip (1).exe
webfile:_...\fzip (1).exe|https://firebasestorage.googleapis.com/.../fzip.exe
```

That `webfile:` entry is the reputation path. Defender's cloud is asked about a
file it has never seen, from a generic file-hosting URL, with no signature. The
`!ml` suffix means the answer came from a machine-learning model making a
probability estimate — not from a signature matching known malicious code.

This path is *non-deterministic across builds*, which the testing confirmed:
recompiling the identical source produced a binary the same engine accepted.

## The one real defect found

Fzip's RAR support compiles RARLAB's UnRAR C++ sources. Those sources are the
**whole WinRAR utility**, not just the extractor. They contain
`Shutdown(POWER_MODE)`, `IsUserAdmin()` and `GetRarDataPath()`, which drag these
Windows imports into the executable:

| Import | Comes from | Does Fzip call it? |
|---|---|---|
| `AdjustTokenPrivileges` | UnRAR `Shutdown()` acquiring `SeShutdownPrivilege` | **No** |
| `OpenProcessToken` | same | **No** |
| `LookupPrivilegeValueW` | same | **No** |
| `AllocateAndInitializeSid` | UnRAR `IsUserAdmin()` | **No** |

Now consider what a classifier sees. Fzip walks a directory tree recursively,
reads every file, encrypts each with AES-256, and writes the results — and the
same binary imports the API sequence used to elevate privileges. That is the
import-plus-behaviour fingerprint of ransomware. Fzip's intent is different, but
intent is not in the binary.

Verified by building both ways and comparing the import tables:

```
Windows API                CO RAR    KHONG RAR
AdjustTokenPrivileges      present   absent
OpenProcessToken           present   absent
AllocateAndInitializeSid   present   absent
LookupPrivilegeValueW      present   absent
```

## What was done about it

**RAR support is now an opt-out compile-time feature.** Building without it
removes all four privilege imports and 682 KB of unreachable C++:

```bash
cargo build --release                        # 2,865,664 bytes, reads RAR
cargo build --release --no-default-features  # no RAR, none of those imports, 682 KB smaller
```

A build without RAR says so honestly in `-V` and `-h`, and refuses a `.rar` file
with a clear message rather than pretending.

Earlier hardening, still in place: full version resource naming Tcoder LLC, an
application manifest (`asInvoker`, supported-OS list, long-path and UTF-8
declarations), and Control Flow Guard. Section entropy is 6.3 — normal compiled
code. Fzip is not packed or obfuscated, and never will be: packing is itself a
strong malware signal, and hiding from scanners would be the wrong thing to do.

## The remaining gap, stated plainly

None of the above reliably ends the warning, because the dominant factors are
**no code signature** and **zero prevalence**. Everything else is noise around
those two.

An Authenticode certificate issued to Tcoder LLC is the actual fix:

- **OV** certificate, roughly US$200–400/year: removes "unknown publisher";
  SmartScreen reputation still builds over time.
- **EV** certificate, more expensive plus a hardware token: SmartScreen
  reputation is granted immediately.
- **Self-signed**: useless here, and installing one into the root store weakens
  the machine that does it. Do not do this to silence a warning.

## Reporting the false positive

This is free, takes two minutes, and the correction reaches every Defender user:

<https://www.microsoft.com/en-us/wdsi/filesubmission> — choose **Software
developer** and **Incorrectly detected**. Legitimate software is usually
corrected within a few days.

## Verifying a build yourself

```powershell
Get-FileHash fzip.exe -Algorithm SHA256
```

Release 1.0.0:

| Build | SHA256 | Size |
|---|---|---|
| Published release | `28157C577F91D141897259D5C68CA1F92A9C06F557B14189E96899DB48550E09` | 2,865,664 |

> That hash identifies **the published binary**. Rust builds are not
> bit-for-bit reproducible — the compiler embeds absolute paths and other
> build-machine details — so compiling the source yourself yields a functionally
> identical executable with a different hash. That is expected.

Stronger checks:

- **[VirusTotal](https://www.virustotal.com/)** — a real threat is flagged by
  most engines; a false positive by one or two heuristic ones.
- **Build it yourself** with `cargo build --release`. Every dependency is a
  published crate pinned in `Cargo.lock`. This removes the need to trust the
  binary at all, and is the strongest check available without a signature.

Only consider a scanner exclusion after you have done one of the above, and
never disable real-time protection outright.

## Note on the azcopy comparison

Fzip is compared to Microsoft's `azcopy.exe` for one reason: the **distribution
model** — a single self-contained executable, no installer, no runtime, no
registry footprint, driven from the command line. azcopy is written in **Go**;
Fzip is written in **Rust**. As the testing above shows, that difference is not
why one is flagged and the other is not — azcopy is signed by Microsoft and has
been downloaded millions of times. Signature and prevalence are the whole story.
