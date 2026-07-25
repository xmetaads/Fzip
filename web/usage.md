# Fzip — command reference

Version 2.0.0 · Windows 10 and 11, 64-bit · Published by Tcoder LLC · MIT licence

Fzip is a command-line archive tool. It has no graphical interface: you type a
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

## Formats

**Reads:** zip. **Writes:** zip, with optional AES-256.

That is the whole list. Version 1.x also read rar, 7z, tar, gz, bz2, xz and zst;
version 2.0 dropped them. If you hand Fzip one of those it names the format and
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
| 1 | 0.947 s | 181 MB/s | 1.00× |
| 2 | 0.540 s | 318 MB/s | 1.76× |
| 4 | 0.271 s | 633 MB/s | 3.50× |
| 8 | 0.182 s | 944 MB/s | 5.22× |
| 12 | 0.149 s | 1151 MB/s | 6.36× |
| 20 | 0.132 s | 1304 MB/s | 7.20× |

Seven times the throughput of a single worker, and still climbing at twenty
because the archive holds 360 independent entries to share out.

Extracting the same archive to an SSD takes about 0.70 s — roughly 245 MB/s end
to end, against 119 MB/s on a single worker. Writing sets the pace there, not
decompression.

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
  is about 7 MB regardless of archive size. The exception is an encrypted entry,
  whose authentication code covers the whole payload and so has to be held in
  memory to be checked; `--max-memory` caps that.
- **No third-party code.** Fzip depends on nothing outside the Go standard
  library, so there is no supply chain to compromise.

---

## Verifying the download

```powershell
Get-FileHash fzip.exe -Algorithm SHA256
```

Version 2.0.0:

```
SHA-256  PLACEHOLDER_SHA256
Size     PLACEHOLDER_SIZE bytes
```

That hash identifies the published binary. Compiling the source yourself may
produce a different hash — that is expected, not a sign of tampering.

Fzip has **no third-party dependencies**, so building it yourself means compiling
this repository against the Go standard library and nothing else. The executable
also records where it came from:

```powershell
go version -m fzip.exe
```

That prints the module path and the exact commit the binary was built from.

### If Windows Defender blocks the download

Fzip is not yet code-signed. An unsigned executable that a scanner has never seen
before carries no reputation, and Defender's cloud model sometimes returns a
malicious verdict on that basis alone — usually `Trojan:Win32/Wacatac.B!ml`.
Version 1.0.1 was blocked this way.

The verdict attaches to a **specific file hash**, not to the program. We measured
this: the published 1.0.1 file was blocked on every download, while the identical
source rebuilt locally scanned clean, and one local build was detected and then
scanned clean twenty minutes later without changing a byte. Six builds across two
different configurations made no difference. Changing implementation language did
not either — that was measured too, on a minimal program in each.

If it happens to you:

1. Check the SHA-256 above. If it matches, you have the file we published.
2. Report it to Microsoft as a false positive at <https://aka.ms/wdsi>. This is
   what actually gets the verdict corrected, and it helps everyone after you.
3. Only then decide about an exclusion, scoped to the single file.

Never turn off real-time protection to run an archive tool. If the hash does not
match, do not run the file at all — tell us instead.
