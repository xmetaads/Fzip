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
| "Fzip imports privileged APIs it never uses" | **True, but not the trigger** | The imports are real and are described below. Six controlled builds on 2026-07-25 showed they make no difference to the verdict. |
| "Mark-of-the-Web is what condemns the download" | **Ruled out** | The blocked 1.0.1 file was rejected before any zone stream was written to it. |

## How the detection first reached the user

In the first round, the Defender record showed the file flagged on **download**
rather than on local execution. (In the 1.0.1 recurrence documented below, local
files were flagged too, so this is not a rule — but the reputation path described
here is still the mechanism.)

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

## The 1.0.1 recurrence, and the experiment that settled it

Version 1.0.1 was blocked again on 2026-07-25. This time it reproduced on demand,
which made a controlled comparison possible. Security Intelligence
**1.455.339.0**, engine 1.1.26060.3008, cloud protection Advanced, Block at First
Sight on.

| Test | Result |
|---|---|
| Published 1.0.1 file, downloaded twice, 20 minutes apart | blocked both times |
| Same file, before any Mark-of-the-Web stream was applied | blocked |
| Same source rebuilt locally, different hash | clean |
| Local build with a browser-equivalent Mark-of-the-Web applied | clean |
| Three builds **with** RAR, three distinct hashes | clean, 3/3 |
| Three builds **without** RAR, three distinct hashes | clean, 3/3 |

Two entries from `Get-MpThreatDetection` finish the argument:

- A local build was detected at 19:51:11 and **the same bytes on the same
  machine** scanned clean at 20:11. Nothing about the file changed in between.
- Right after the download was blocked at 19:50:02, two freshly compiled binaries
  with unrelated hashes were also detected, at 19:50:18 and 19:51:11, and later
  cleared. A verdict appears to propagate across similar files and then expire.

So the verdict tracks **a specific file hash**. Not the language, not the
dependencies, not Mark-of-the-Web, and not UnRAR — which is worth stating plainly
because UnRAR had been the leading suspect in the previous round of this
investigation, and that suspicion was wrong.

The response is therefore to retire the condemned hash by publishing a rebuild
(1.0.2) and to report the old hash to Microsoft. Neither is a cure. See "The
remaining gap, stated plainly" below.

## A real wart, though not the cause

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

Consider what a classifier sees. Fzip walks a directory tree recursively, reads
every file, encrypts each with AES-256, and writes the results — and the same
binary imports the API sequence used to elevate privileges. That is the
import-plus-behaviour fingerprint of ransomware. Fzip's intent is different, but
intent is not in the binary.

That reasoning is sound and the imports are genuinely unwanted, which is why the
opt-out below exists. It is simply not what produced the detections: measured
head to head, builds with and without those imports are equally clean.

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
cargo build --release                        # 2,874,368 bytes, reads RAR
cargo build --release --no-default-features  # no RAR, none of those imports, 682 KB smaller
```

A build without RAR says so honestly in `-V` and `-h`, and refuses a `.rar` file
with a clear message rather than pretending.

Earlier hardening, still in place: full version resource naming Tcoder LLC, an
application manifest (`asInvoker`, supported-OS list, long-path and UTF-8
declarations), and Control Flow Guard. Section entropy is 6.3 — normal compiled
code. Fzip is not packed or obfuscated, and never will be: packing is itself a
strong malware signal, and hiding from scanners would be the wrong thing to do.

## What Microsoft said

The binary was submitted through <https://www.microsoft.com/en-us/wdsi/submission>.
Their analysts replied:

> Our scanners show no positive detection, and we have no telemetry indicators
> for the file(s) submitted either.

That is consistent with everything measured above: there is no signature
matching this file. The verdict came from the cloud model at download time, and
it is not reproducible from the file alone.

They asked for the detection to be reproduced on the latest Security
Intelligence and for `MPSupportFiles.cab` to be collected from the machine that
reported it. Retested on **2026-07-25** with Security Intelligence
**1.455.339.0**, engine 1.1.26060.3008, cloud protection on Advanced and Block
at First Sight enabled:

| Test | Result |
|---|---|
| Download the released binary over HTTPS | 2,874,368 bytes, hash matches |
| Apply a Mark-of-the-Web stream, as a browser download does | — |
| `MpCmdRun.exe -Scan -ScanType 3` | **found no threats** |
| Execute it | runs normally, prints `Fzip 1.0.1` |
| New entries in `Get-MpThreatDetection` | **none** |

**The detection no longer reproduces**, so there is no diagnostic data to
collect. [MICROSOFT-REPLY.md](MICROSOFT-REPLY.md) holds a drafted reply and the
exact commands to run if it returns.

The important thing this does *not* mean: the problem is not permanently solved.
Each new release is a new file hash with zero reputation, so the same
machine-learning path can reach the same conclusion again. Only a code signature
breaks that cycle.

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

Release 1.0.1:

| Build | SHA256 | Size |
|---|---|---|
| Published release | `EA01A7B0041541B43CABD747594A4A93073A8113523F6CE45844E09894C30552` | 2,874,368 |

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
