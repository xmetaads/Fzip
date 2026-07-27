# Fzip — command reference

Version 1.4.0 · Windows 10 and 11, 64-bit · Published by Tcoder LLC · MIT licence

Fzip is a command-line zip tool. It has no graphical interface: you type a
command, it does the work and prints what happened. It decompresses every file
in an archive at the same time, one worker per CPU core.

---

## Before the first command

**Double-clicking `fzip.exe` shows a help screen and waits for Enter.** That is
expected — there is no window to open. There are three ways to use it:

**Drag and drop.** Drag an archive onto `fzip.exe` in File Explorer. It unpacks
into a folder beside the archive.

**A terminal in the right folder.** Open the folder containing your archive in
File Explorer, click the address bar, type `cmd`, press Enter. A terminal opens
already pointed at that folder.

**On your PATH.** Put `fzip.exe` somewhere permanent such as `C:\Tools`, then add
that folder to your PATH. Run this once in PowerShell and reopen the window:

```powershell
$dir = "C:\Tools"
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item .\fzip.exe $dir -Force
$old = [Environment]::GetEnvironmentVariable("Path", "User")
if ($old -notlike "*$dir*") {
  [Environment]::SetEnvironmentVariable("Path", "$old;$dir", "User")
}
```

After that, `fzip` works from any directory.

---

## Commands

```
fzip x <archive> [options]        extract
fzip l <archive> [options]        list contents
fzip t <archive> [options]        test integrity, write nothing
fzip a <archive.zip> <paths...>   create a zip
fzip <archive>                    same as x — this is what drag-and-drop uses
fzip -h                           help
fzip -V                           version
```

### `x` — extract

```
fzip x data.zip
```

Creates a folder named after the archive, **next to the archive**, and unpacks
into it. `D:\files\report.zip` becomes `D:\files\report\`.

This matters for drag-and-drop: the working directory then belongs to Explorer,
not to you, so output going "next to the archive" is the predictable choice.

```
fzip x data.zip -o D:\output      choose the destination
fzip x data.zip -e                flatten: ignore the folder structure
fzip x secret.zip -p MyPass123    password inline
fzip x secret.zip -p              prompt for the password instead
```

### `l` — list

```
fzip l data.zip
```

Prints every entry with its uncompressed size, packed size, compression method
and encryption scheme. Writes nothing to disk. Useful for checking whether an
archive is encrypted before you commit to extracting it.

### `t` — test

```
fzip t data.zip
```

Decompresses every entry in memory and verifies its CRC, without writing
anything. Use it on a download before you trust it, or on an archive that has
been sitting in storage. Exit code `0` means every entry verified.

### `a` — create

```
fzip a backup.zip MyFolder
fzip a backup.zip Folder1 Folder2 notes.txt    several inputs at once
fzip a backup.zip MyFolder -mx9                smallest output
fzip a backup.zip MyFolder -mx0                store, no compression
fzip a backup.zip MyFolder -p                  prompt for a password, AES-256
fzip a backup.zip MyFolder -y                  overwrite an existing archive
```

Fzip refuses to overwrite an existing archive unless you pass `-y`. Source files
are only ever read, never moved or deleted.

---

## Options

| Option | Meaning |
|---|---|
| `-o <dir>` | Output folder. Default: a folder named after the archive, created beside it. |
| `-p [password]` | Password. Give it inline, or leave the value off and Fzip prompts without echoing — which keeps the password out of your shell history. |
| `-t <n>` | Worker threads. Default: every core. Maximum 1024. |
| `-i <glob>` | Include only entries matching the pattern. Repeatable. |
| `-x <glob>` | Exclude entries matching the pattern. Repeatable. |
| `-e` | Flatten: ignore folder structure, put every file in one place. |
| `-y` | Assume yes. Required to overwrite an existing archive with `a`. |
| `--overwrite <mode>` | How to treat files that already exist: `all` (default), `skip`, `rename`, `newer`. |
| `-mx<0-9>` | Compression level for `a`. `0` stores without compressing, `5` is the default, `9` is smallest. |
| `--max-memory <n>` | RAM ceiling, e.g. `512M` or `2G`. Default `1G`. |
| `--no-crc` | Skip CRC verification. Slightly faster, less safe. |
| `--progress` | Force the progress bar even when output is redirected to a file. |
| `--no-pause` | Never wait for a keypress at the end. Set it for installers and scripts; the environment variable `FZIP_NO_PAUSE` does the same. |
| `-q` | Quiet: errors only. |
| `-v` | Verbose: name every file as it is processed. |

### The four overwrite modes

| Mode | When the destination file already exists |
|---|---|
| `all` | Overwrite it. This is the default. |
| `skip` | Leave the existing file alone. |
| `rename` | Keep the existing file; write the new one as `name_1.txt`, `name_2.txt`, … |
| `newer` | Overwrite only if the archived copy has a newer timestamp. |

---

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Warning — something was skipped, everything else succeeded. |
| `2` | Error — corrupt archive, wrong password, or could not write. |
| `7` | Bad command line, or the file was not found. |

Batch script:

```bat
@echo off
fzip x "%~1" -q
if errorlevel 2 (
    echo Extraction failed
    exit /b 1
)
echo Done
```

PowerShell:

```powershell
fzip x $Archive -o $Dest -q
if ($LASTEXITCODE -ge 2) { throw "fzip failed with exit code $LASTEXITCODE" }
```

---

## Installers, services and scheduled tasks

`fzip.exe` is a console program, so when an MSI custom action or a service
launches it, Windows allocates a console and a black window flashes for a
fraction of a second. That is Windows doing what it is told, not a fault in
Fzip — but in an enterprise installer it looks like one.

Ship **`fzipw.exe`** instead. It is the same program with one field changed in
the executable header: no console is ever allocated, so no window can appear.
The convention is Windows' own — `python.exe` / `pythonw.exe`,
`java.exe` / `javaw.exe`.

Download it at **<https://fzip.org/download/fzipw.exe>**.

```
fzipw x payload.zip -o "%INSTALLFOLDER%"
```

Everything else is identical: same options, same exit codes, same archives.

**Two things to know before you script it.**

It prints nothing unless you redirect. With no console there is nowhere for
output to go, so check the exit code — which is what an installer should do
anyway. To capture a log, redirect and it reappears:

```
fzipw x payload.zip -o D:\app > install.log 2>&1
```

And **PowerShell does not wait for it.** Called plainly, PowerShell starts it and
moves to the next line immediately, so `$LASTEXITCODE` means nothing and the
files are not there yet. Ask for the wait:

| Invocation | Waits | Exit code |
|---|---|---|
| `& fzipw x a.zip -o out` | no | meaningless |
| `& fzipw x a.zip -o out \| Out-Null` | yes | correct |
| `Start-Process fzipw -ArgumentList ... -Wait -PassThru` | yes | correct |
| `cmd /c fzipw x a.zip -o out` | yes | correct |

MSI custom actions, Task Scheduler, services and .NET's `Process.WaitForExit`
all wait correctly on their own — they hold the process handle. The caveat is
specific to calling it from a shell.

**If you would rather not ship a second file:** WiX can suppress the window
itself. `WixQuietExec` (in `WixUtilExtension`) runs a console program with
`CREATE_NO_WINDOW`, and nothing appears. That is the supported route and needs
no change on our side; `fzipw.exe` is for when you do not control the caller.

---

## Formats

**Reads:** zip. **Writes:** zip, with optional AES-256.

That is the whole list. Fzip 1.0 also read rar, 7z, tar, gz, bz2, xz and zst;
1.2 dropped them. If you hand Fzip one of those it names the format and
stops, rather than reporting a corrupt zip:

```
> fzip x archive.rar
fzip: archive.rar is a RAR archive, and this version reads zip only
```

Within zip it handles deflate and stored entries, ZIP64, archives with more than
65535 entries, and both UTF-8 and legacy CP437 entry names.

Format is detected from the magic bytes at the start of the file, never from the
extension, so a zip named `.rar` still opens correctly.

**Encryption it can read:** AES-256, AES-192 and AES-128 (the WinZip AE-1 and
AE-2 schemes), and legacy ZipCrypto.

**Encryption it writes:** AES-256 only. Fzip deliberately will not create legacy
ZipCrypto — that cipher is broken — so asking for a password always gives you
real encryption.

**Zip methods not supported:** BZip2, LZMA, Zstd, XZ and Deflate64 inside a zip.
All are reported clearly rather than mis-extracted. Deflate and stored account
for essentially every zip in circulation.

---

## Common tasks

```
# Unpack a download and check it first
fzip t installer.zip
fzip x installer.zip

# Pull only the images out of a large archive
fzip x photos.zip -i "*.jpg" -i "*.png" -o D:\Images

# Unpack without the junk
fzip x project.zip -x "*.tmp" -x "*.log"

# Add files to a folder without touching what is already there
fzip x update.zip -o D:\app --overwrite skip

# Collapse a nested archive into one flat folder
fzip x docs.zip -e -o D:\Flat

# Encrypted backup, smallest possible
fzip a Backup-2026.zip D:\Work -mx9 -p

# See whether an archive is encrypted before extracting
fzip l archive.zip

# Limit Fzip to four cores on a busy machine
fzip x huge.zip -t 4
```

---

## Performance

A ZIP archive stores each file as an independent compressed block, so they can
all be decompressed at once. Measured on a 20-thread laptop CPU (AMD Ryzen AI 9
465) with a 171.7 MB archive of 360 files, using `fzip t` so that disk writes do
not distort the result. Five runs at each worker count, best taken:

| Workers | Time | Throughput | Versus one worker |
|---|---|---|---|
| 1 | 0.287 s | 599 MB/s | 1.00× |
| 2 | 0.158 s | 1087 MB/s | 1.81× |
| 4 | 0.084 s | 2040 MB/s | 3.41× |
| 8 | 0.094 s | 1817 MB/s | 3.03× |
| 12 | 0.098 s | 1757 MB/s | 2.93× |
| 20 | 0.091 s | 1892 MB/s | 3.16× |

Three and a half times a single worker, levelling off at four. Past that the
whole archive finishes in under a tenth of a second and thread scheduling costs
more than it saves, which is why the last three rows wobble instead of climbing.

Extracting the same archive to an SSD takes about 0.54 s — roughly 318 MB/s end
to end, against 277 MB/s on a single worker. Writing sets the pace there, not
decompression, which is why the gap nearly disappears.

**Where this does not help:** an archive holding one enormous entry has nothing
to parallelise, and neither does extraction to slow storage. Fzip's advantage
shows up with many entries and a fast disk, or with `fzip t`, which writes
nothing at all.

---

## Safety

Every item below is covered by an automated regression test.

- **Path traversal is blocked.** Entries containing `..`, drive letters or
  absolute roots cannot write outside the destination. Each blocked entry is
  reported by name rather than silently dropped.
- **Zip bombs are refused.** An entry that expands past its declared size is
  stopped one byte over, so a 199 KB archive claiming 1 KB cannot quietly write
  200 MB.
- **Reserved device names are renamed.** An entry called `CON`, `NUL`, `LPT1` or
  `PRN.txt` becomes `_CON` and so on; files genuinely named after a device are
  nearly impossible to delete afterwards.
- **Long paths work.** Beyond the 260-character limit Fzip writes through the
  extended form, and never reports success for a file it failed to create.
- **AES is authenticated before decryption**, so tampered data is rejected rather
  than decoded into garbage.
- **CRC is verified on every entry** by default.
- **Memory stays flat.** Entries above 32 MB stream straight to disk; peak usage
  is about 7.5 MB regardless of archive size. The exception is an encrypted entry,
  whose authentication code covers the whole payload and so has to be held in
  memory to be checked; `--max-memory` caps that.
- **Nothing privileged is imported.** No `AdjustTokenPrivileges`, no process
  injection, no registry writing. The import table is five Windows system DLLs,
  and the C runtime is linked in rather than imported, so there is no Visual C++
  Redistributable to install.

---

## Verifying the download

```powershell
Get-FileHash fzip.exe -Algorithm SHA256
```

Version 1.4.0:

```
fzip.exe   SHA-256  F9ED71C6F04FB44C7BBCC54BBC543AC40E919ECEB3A921B0825ECC91CB81FF4D
           Size     1,613,824 bytes

fzipw.exe  SHA-256  25EB5AE409BC6C2A52DD9353E36717E073BCD9CD233851A51B87213E90D20D2F
           Size     1,613,312 bytes
```

Download links:

- `fzip.exe` — <https://fzip.org/download/fzip.exe>
- `fzipw.exe` — <https://fzip.org/download/fzipw.exe>

Pinned to this version, so a link in your build script keeps meaning one build:

- <https://fzip.org/download/fzip-1.4.0-x64.exe>
- <https://fzip.org/download/fzipw-1.4.0-x64.exe>

This exact file was submitted to Microsoft Security Intelligence, analysed, and
**allow-listed**. Microsoft Defender no longer flags it and SmartScreen passes
it. Checking the hash is therefore worth doing: it tells you that you have the
reviewed build and not something else.

Fzip is **not code-signed**. What vouches for it is that review, the published
SHA-256, and source you can read and rebuild — not a certificate. That is said
plainly here because claiming a signature you cannot verify would be worse than
claiming nothing.

Compiling the source yourself produces a functionally identical executable with
a *different* hash, because Rust builds are not bit-for-bit reproducible — that
is expected, not a sign of tampering. Every dependency is pinned in
`Cargo.lock`.

### If a scanner still flags it

Earlier releases drew `Trojan:Win32/Wacatac.B!ml` from Microsoft Defender. That
was a machine-learning false positive, and it has been corrected for this build.

It is worth knowing what the investigation found, because the same thing can
happen to any small unsigned tool. The verdict attached to a **specific file
hash**, not to the program: the published 1.0.1 file was blocked on every
download while the identical source rebuilt locally scanned clean, and one local
build was detected and then scanned clean twenty minutes later without a byte
changing. Six builds across two configurations made no difference, and neither
did changing implementation language — both were measured.

If some other scanner flags it:

1. Check the SHA-256 above. If it matches, you have the file we published.
2. Report it to that vendor as a false positive. For Microsoft the address is
   <https://aka.ms/wdsi>; that is what actually gets a verdict corrected, and it
   helps everyone after you.
3. Only then consider an exclusion, scoped to the single file.

Never turn off real-time protection to run an archive tool. If the hash does not
match, do not run the file at all — tell us instead.
