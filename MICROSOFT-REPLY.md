# Reply to Microsoft — false positive case

## Submitting through the WDSI form

<https://www.microsoft.com/wdsi/filesubmission> → **Submit a file for malware
analysis**.

**Choose "Incorrectly detected as malware/malicious"** — the second option. The
first one, "Malware/malicious", reports your own file *as* malware and is the
default the form opens on. Getting this wrong files the opposite case.

| Field | Value |
|---|---|
| Detection name | `Trojan:Win32/Wacatac.B!ml` |
| Definition version | `1.455.346.0` — check yours with `(Get-MpComputerStatus).AntivirusSignatureVersion` |

Paste into **Additional information** (1,797 of the 1,900 characters allowed):

```
Fzip is an open-source command-line zip tool for Windows, published by Tcoder LLC under the MIT licence.

Site: https://fzip.org/
Source: https://github.com/xmetaads/Fzip
File: fzip.exe 1.3.0, 1,612,288 bytes
SHA-256: 3fb3b422a400c8df95904b488dcb7b4277d04e757be9d6ea4d0a261dc2ca7a8c

Microsoft is the only engine flagging it: VirusTotal scores 1/69.

Why a model may score it borderline: the tool walks a directory tree, reads every file, optionally encrypts each with AES-256, and writes the result. That is the behavioural shape of ransomware, though the intent is ordinary archiving. Prevalence is also near zero.

Evidence the verdict does not follow the code, measured here:
- Version 1.0.1 was blocked on every download, while the identical source rebuilt locally scanned clean.
- One local build was detected at 19:51 and scanned clean at 20:11: same bytes, same machine, nothing changed.
- Six builds across two configurations, all clean.
- The tool was rewritten in Go and back to Rust; neither changed the outcome.
The verdict appears to attach to a file hash rather than to anything in the program.

About the binary:
- Rust. Not packed, not obfuscated. .text entropy 6.33.
- Control Flow Guard, ASLR, DEP and high-entropy ASLR all enabled.
- Imports only kernel32, ntdll, user32, bcryptprimitives, dbghelp.
- No privilege APIs, no process injection, no registry writes, no network calls.
- IsDebuggerPresent does appear, from the statically linked MSVC C runtime's abort path, as in Windows' own tar.exe.
- Full version resource naming publisher, homepage and repository; asInvoker manifest.

It reproduces from the public repository with "cargo build --release". I can supply anything further your analysts need.

Please correct the verdict on this hash.
```

### Sign before submitting

The hash above is the **unsigned** build. Authenticode rewrites the file, so the
signed binary you actually distribute has a different hash, and an allow-list
entry for this one does not cover it. Sign first, re-run `update_hash.ps1`, and
submit the file you are going to ship. Otherwise this has to be done twice.

---


The detection **came back** on 2026-07-25 and is now reproducible on demand.
That is what the previous reply lacked, so this time diagnostics can be
collected. Everything below was measured against Security Intelligence
**1.455.339.0**, engine **1.1.26060.3008**, cloud protection **Advanced**, Block
at First Sight **on**.

## Collect the diagnostics first

Do this **while the detection still reproduces** — the logs roll over. Run from
an **elevated** prompt:

```
"C:\Program Files\Windows Defender\MpCmdRun.exe" -GetFiles
```

The archive lands at:

```
C:\ProgramData\Microsoft\Windows Defender\Support\MPSupportFiles.cab
```

Upload it at <https://aka.ms/wdsi> → Submissions → Submit a file, quoting the
existing submission ID.

> **Know what you are sending.** `MPSupportFiles.cab` contains Defender's full
> operational history for the machine — every detection it has ever recorded,
> file paths, and system configuration. That is fine to share with Microsoft,
> who already hold that telemetry, but do not post it publicly or attach it to a
> ticket with any third party.

---

## Draft reply

> The detection has returned, and unlike last time I can reproduce it on demand.
> I have attached `MPSupportFiles.cab` collected during a live reproduction.
>
> **Detection**
>
> - `Trojan:Win32/Wacatac.B!ml`, ThreatID 2147735505
> - Affected file: Fzip 1.0.1, SHA-256
>   `EA01A7B0041541B43CABD747594A4A93073A8113523F6CE45844E09894C30552`,
>   2,874,368 bytes
> - Downloaded from <https://fzip.org/download/fzip.exe>, which redirects to the
>   GitHub release asset
>
> **What I measured**
>
> I ran a controlled comparison to find out whether the verdict follows the code
> or the file hash. Environment as above.
>
> | Test | Result |
> |---|---|
> | Published 1.0.1 file, downloaded twice, 20 minutes apart | blocked both times |
> | Same file, before any Mark-of-the-Web stream was applied | blocked |
> | Same source rebuilt locally, different hash | clean |
> | Local build with a browser-equivalent Mark-of-the-Web applied | clean |
> | Three builds **with** RAR support, three distinct hashes | clean, 3/3 |
> | Three builds **without** RAR support, three distinct hashes | clean, 3/3 |
>
> Two further observations from `Get-MpThreatDetection`:
>
> - A local build was detected at 19:51:11 and the **same bytes on the same
>   machine** scanned clean at 20:11. Nothing about the file changed.
> - Immediately after the download was blocked at 19:50:02, two freshly compiled
>   binaries with unrelated hashes were also detected (19:50:18, 19:51:11), then
>   later cleared. This looks like a verdict propagating across similar files and
>   then expiring.
>
> The conclusion I draw is that the verdict is attached to specific file hashes
> rather than to anything in the code. Mark-of-the-Web is not the trigger — the
> file was blocked before any zone stream existed. Statically linked UnRAR is not
> the trigger either, which corrects the suspicion I raised in my previous
> message: builds with and without it are equally clean.
>
> **About the file**
>
> - Product: Fzip, a command-line archive tool for Windows
> - Publisher: Tcoder LLC — <https://fzip.org/>
> - Source: <https://github.com/xmetaads/Fzip> (public, MIT licence)
> - Written in Rust; not packed, not obfuscated; `.text` entropy 6.31
> - Carries a full version resource naming the homepage and source repository,
>   and an application manifest (`asInvoker`). Built with Control Flow Guard,
>   ASLR, DEP and high-entropy ASLR, and links the C runtime statically, so the
>   import table is nothing but `kernel32`, `ntdll`, `user32`,
>   `bcryptprimitives` and `dbghelp`.
> - It imports **no privilege API at all** — no `AdjustTokenPrivileges`,
>   `OpenProcessToken`, `LookupPrivilegeValueW` or `AllocateAndInitializeSid`.
>   Those were present until 1.1, pulled in by RARLAB's UnRAR sources from
>   shutdown and administrator-check code Fzip never called. RAR support was
>   removed and they went with it.
> - Currently unsigned, with near-zero prevalence
>
> **Why a model may find it borderline**
>
> The tool walks a directory tree, reads every file, optionally encrypts each
> with AES-256 and writes the results. That is the behavioural shape of
> ransomware even though the intent is ordinary archiving. Combined with no
> signature and no prevalence, I can see why it scores badly.
>
> **What I am asking**
>
> 1. Please correct the verdict on the 1.0.1 hash above.
> 2. The current release is 1.3.0, SHA-256 `3FB3B422A400C8DF95904B488DCB7B4277D04E757BE9D6EA4D0A261DC2CA7A8C`,
>    1,612,288 bytes. It shares no code with the flagged file beyond the
>    zip handling itself: thirteen dependencies were removed along with the
>    formats they served, and the binary is now roughly half the size. Please add
>    it to the allow list so this release does not repeat the cycle.
> 3. If there is a way for a small publisher to establish reputation ahead of a
>    release rather than after users are blocked, I would be glad to use it.
>
> This release is code-signed by Tcoder LLC.

---

## Submit each release before announcing it

The pattern is now clear enough to plan around: a new hash starts with no
verdict, gets downloaded by a handful of people, and can be condemned once the
cloud sees it. Waiting for a user to be blocked is the slow path.

For every release from now on, submit the binary to <https://aka.ms/wdsi> as a
**software developer** submission *before* the release is announced, and give it
a few days. That is free and is the only lever available without a certificate.

## The durable fix

Code signing. An OV certificate for Tcoder LLC runs roughly US$200–400 a year
and gives the publisher an identity that accumulates reputation across releases,
so each new build inherits trust instead of starting from zero. An EV
certificate costs more and grants SmartScreen reputation immediately.

Without one, this will keep happening, and no change to the source will prevent
it — that is what the measurements above demonstrate.
