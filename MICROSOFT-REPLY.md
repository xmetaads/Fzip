# Reply to Microsoft — false positive case

Paste the section below into a reply on the WDSI case thread. Everything in it
was verified on 2026-07-25 against Security Intelligence **1.455.339.0**.

---

## Draft reply

> Thank you for looking at this.
>
> I have completed step 1 of your instructions and can confirm the detection no
> longer reproduces.
>
> **Environment**
>
> - Windows 11, Defender platform 4.18.26060.3008, engine 1.1.26060.3008
> - Security Intelligence updated to **1.455.339.0** before retesting
> - Cloud-delivered protection: Advanced (MAPSReporting = 2)
> - Block at First Sight: enabled
>
> **What was originally detected**
>
> - `Trojan:Win32/Wacatac.B!ml`, ThreatID 2147735505
> - The record shows it fired on the **download path**, not on execution:
>   `webfile:_...\fzip (1).exe|https://firebasestorage.googleapis.com/.../fzip.exe`
> - A second build of the identical source was detected on a local scan and
>   quarantined; a third build of that same source scanned clean. The verdict
>   therefore attached to the file hash rather than to the code.
>
> **Retest after updating**
>
> I downloaded the current release over HTTPS, applied a Mark-of-the-Web
> Zone.Identifier stream matching a real browser download, scanned it and ran it:
>
> - `MpCmdRun.exe -Scan -ScanType 3` → *found no threats*
> - The file executes normally
> - No new entries in `Get-MpThreatDetection`
>
> This matches your analysts' result, so I have no live reproduction to collect
> `MPSupportFiles.cab` from. I will run `mpcmdrun.exe -GetFiles` and submit it
> immediately if the detection returns.
>
> **About the file**
>
> - Product: Fzip 1.0.0, a command-line archive tool for Windows
> - Publisher: Tcoder LLC — <https://fzip.org/>
> - Source: <https://github.com/xmetaads/Fzip> (public, MIT licence)
> - Download: <https://fzip.org/download/fzip.exe>
> - SHA-256: `28157C577F91D141897259D5C68CA1F92A9C06F557B14189E96899DB48550E09`
> - Size: 2,865,664 bytes
> - Written in Rust; not packed, not obfuscated; section entropy 6.3
> - Carries a full version resource, an application manifest (`asInvoker`), and
>   is built with Control Flow Guard, ASLR, DEP and high-entropy ASLR
>
> **Why I believe a model finds it borderline**
>
> The tool walks a directory tree, reads every file, optionally encrypts each
> with AES-256 and writes the results — the behavioural shape of ransomware,
> although the intent is ordinary archiving. It is also currently unsigned and
> has near-zero prevalence, which I understand weighs heavily in the cloud
> verdict.
>
> One contributing factor I have already identified: RAR support statically links
> RARLAB's UnRAR sources, which bring `AdjustTokenPrivileges`, `OpenProcessToken`,
> `LookupPrivilegeValueW` and `AllocateAndInitializeSid` into the import table
> from `Shutdown()` and `IsUserAdmin()` code paths that Fzip never calls. A build
> configured without RAR has none of those imports. I can supply both builds for
> comparison if that is useful to your analysts.
>
> I would be grateful if the current binary could be added to your allow list so
> that the next release does not start from zero reputation again.

---

## Before sending

Attach both binaries if you want the analysts to compare import tables:

```powershell
cargo build --release
Copy-Item target\release\fzip.exe fzip-with-rar.exe
cargo build --release --no-default-features
Copy-Item target\release\fzip.exe fzip-no-rar.exe
```

## If the detection comes back

Reproduce it first, then collect diagnostics **within the same session** — the
logs are what Microsoft needs, and they roll over.

Run from an **elevated** Command Prompt or PowerShell:

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
> file paths, and system configuration. On this machine that history includes
> detections on unrelated downloads. That is fine to share with Microsoft, who
> already hold that telemetry, but do not post the archive publicly or attach it
> to a support ticket with any third party.
