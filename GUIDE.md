# Fzip — detailed user guide

**Published by Tcoder LLC.** MIT licensed.

The [README](README.md) covers what Fzip is and how fast it is. This guide covers how to actually drive it day to day.

---

## 1. It is a command line tool

**Double-clicking `fzip.exe` shows a help screen and waits for Enter.** That is not a failure — Fzip has no graphical interface, the same way Microsoft's `azcopy.exe` has none.

There are three ways to use it, easiest first:

| Approach | Best when |
|---|---|
| **Drag and drop** an archive onto `fzip.exe` | You just want it unpacked, no commands to remember |
| Open **cmd / PowerShell** in the folder | You want the full set of options |
| Put it on your **PATH** and type `fzip` anywhere | You use it regularly |

---

## 2. The easy way: drag and drop

Drag a `.zip`, `.rar`, `.7z` (or any supported archive) **onto the `fzip.exe` icon**.

The output folder is created **next to the archive**, named after it with the extension removed:

```
D:\Documents\report.zip     ->   D:\Documents\report\
```

A progress bar appears and the window pauses at `Press Enter to exit...` so you can read the result.

> If the archive is password protected, fzip asks for the password right there. Type it — the characters are hidden — and press Enter.

---

## 3. Opening a prompt in the right folder

This is the most useful skill. Two ways:

**A — from Explorer:** open the folder containing the archive, click the **address bar**, type `cmd` over the path, press Enter. A console opens already pointed at that folder.

**B — Shift + right-click** on empty space in the folder, then *"Open PowerShell window here"*.

Then call fzip using the full path to the executable:

```bash
"C:\Users\admin\Desktop\ung dung giai nen zip\fzip.exe" x report.zip
```

The quotes are **required** because that path contains spaces.

---

## 4. Typing `fzip` from anywhere

Copy `fzip.exe` to a permanent folder and add it to your PATH. Run this **once** in PowerShell:

```powershell
$dest = "C:\Tools"
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item "C:\Users\admin\Desktop\ung dung giai nen zip\fzip.exe" $dest -Force
$current = [Environment]::GetEnvironmentVariable("Path", "User")
if ($current -notlike "*$dest*") {
  [Environment]::SetEnvironmentVariable("Path", "$current;$dest", "User")
}
```

**Close the window and open a new one.** From then on:

```bash
fzip x report.zip
```

---

## 5. The five commands

### `x` — extract

```bash
fzip x report.zip                    # into a "report" folder beside the archive
fzip x report.zip -o D:\Results      # choose the destination
fzip x report.rar                    # rar, 7z, tar.gz ... same command
fzip report.zip                      # shorthand, the "x" is optional
```

### `l` — list contents without extracting

```bash
fzip l report.zip
```

Shows original size, packed size, compression method, encryption, and name.

### `t` — test an archive for damage

```bash
fzip t report.zip
```

Decompresses in memory and verifies every CRC but **writes nothing to disk**. Use it to confirm a download is intact before you rely on it.

### `a` — create a zip

```bash
fzip a backup.zip PhotoFolder                  # compress one folder
fzip a backup.zip Folder1 Folder2 notes.txt    # several inputs at once
fzip a backup.zip Data -mx9                    # maximum compression
fzip a backup.zip Data -p MyPass123            # AES-256 encryption
```

If the target zip already exists, fzip **refuses to overwrite it**. Add `-y` if you really mean to.

### `-V` / `-h` — version and help

```bash
fzip -V
fzip -h
```

---

## 6. Full option reference

| Option | Meaning | Example |
|---|---|---|
| `-o <dir>` | Destination. Default: a folder of the same name, **beside the archive** | `-o D:\Out` |
| `-p <pass>` | Password. Also accepts the attached form `-pMyPass123` | `-p MyPass123` |
| `-p` (alone) | Prompt for the password with hidden input — safer, see below | `-p` |
| `-t <n>` | CPU threads. Default: all of them | `-t 4` |
| `-i <glob>` | Include **only** matching names | `-i "*.pdf"` |
| `-x <glob>` | **Exclude** matching names | `-x "*.tmp"` |
| `-e` | Flatten: drop the folder structure | `-e` |
| `-y` | Assume yes (overwrite the zip when creating) | `-y` |
| `--overwrite <mode>` | Existing files: `all` (default), `skip`, `rename`, `newer` | `--overwrite skip` |
| `-mx<0-9>` | Compression level for `a`. `0` = store, `5` = balanced, `9` = smallest | `-mx9` |
| `--max-memory <n>` | RAM ceiling, e.g. `512M`, `2G`. Default `1G` | `--max-memory 2G` |
| `--no-crc` | Skip CRC checking (marginally faster, less safe) | `--no-crc` |
| `--progress` | Force the progress bar even when redirected to a file | `--progress` |
| `-q` | Quiet, errors only. For scripts | `-q` |
| `-v` | Verbose, print every file name | `-v` |

### The four `--overwrite` modes

| Mode | When the target file already exists |
|---|---|
| `all` | Overwrite it (default) |
| `skip` | Leave the existing file alone |
| `rename` | Keep the old file; write the new one as `name_1.txt`, `name_2.txt`, ... |
| `newer` | Overwrite only if the archived copy is **newer** than the file on disk |

---

## 7. Real-world recipes

**Verify a download before trusting it:**
```bash
fzip t software.zip
fzip x software.zip
```

**Pull only the images out of a huge archive:**
```bash
fzip x photos.rar -i "*.jpg" -i "*.png" -o D:\Images
```

**Extract but leave the junk behind:**
```bash
fzip x project.zip -x "*.tmp" -x "*.log"
```

**Encrypted backup, maximum compression:**
```bash
fzip a Backup-2026.zip D:\Work -mx9 -p
```
Leaving `-p` empty makes fzip **prompt** and hide what you type. That is safer than putting the password on the command line, where it is recorded in your shell history. In a script with no terminal, a bare `-p` is a hard error rather than a silently unencrypted archive.

**Top up an existing folder without touching what is already there:**
```bash
fzip x extras.zip -o D:\Project --overwrite skip
```

**Collect everything into one flat folder:**
```bash
fzip x docs.zip -e -o D:\Flat
```

**Inside a .bat file:**
```bat
@echo off
fzip x "%~1" -q
if errorlevel 2 (
    echo Extraction failed
) else (
    echo Done
)
```

---

## 8. Exit codes for scripting

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Warning — something was skipped, the rest extracted fine |
| `2` | Error — corrupt archive, wrong password, could not write |
| `7` | Bad command line, or the file was not found |

These follow the 7-Zip convention, so existing scripts keep working.

---

## 9. Supported formats

**Extracts:** `.zip` `.rar` `.7z` `.tar` `.gz` `.bz2` `.xz` `.zst`, plus combinations like `.tar.gz`, `.tgz`, `.tar.xz`.

**Creates:** `.zip` only, optionally with AES-256.

Formats are identified by the **magic bytes inside the file**, not by the extension, so a mislabelled archive still extracts correctly.

For `.tar.gz`, fzip notices the tar inside and **unpacks straight to the contents** (like `tar -xzf`) instead of making you do two passes as 7-Zip does.

---

## 10. FAQ

**I double-clicked it and a console appeared with help text.**
That is expected — it pauses at `Press Enter to exit...`. Drag an archive onto the exe instead of double-clicking it.

**The archive has a password and I forgot `-p`.**
fzip asks for it. In a script with no keyboard it reports a clear error instead of hanging.

**It says `wrong password`.**
The password really is wrong. fzip verifies the password against an authentication tag before decrypting, so it never produces garbage files from a bad password.

**It says `SKIPPED (unsafe)`.**
The archive contains an entry that tried to write outside the destination folder (a zip-slip attack) or that is named after a Windows device. fzip blocks it and tells you exactly which entry.

**7z files are slower than in 7-Zip.**
Correct — about 1.3× slower. 7-Zip's LZMA2 decoder is hand-tuned assembly refined over many years, and a solid 7z archive is one continuous stream that cannot be split across cores. If your work is mostly `.7z`, use 7-Zip. fzip's advantage is `.zip`.

**My zip is slightly bigger than 7-Zip's.**
About 2.6% bigger at the same level, in exchange for roughly double the speed. Use `-mx9` if size matters most.

**Does it delete the originals after compressing?**
Never. fzip only reads its input files.
