# =====================================================================
#  fzip - full test suite
#  Usage:  .\run_tests.ps1
#
#  Fzip reads and writes zip, on Windows, and nothing else. Tests for the
#  formats the 1.x releases handled were removed when that code was; what
#  remains of them is section D, which checks that a RAR or 7z file now gets
#  an explanation rather than a puzzling parse error.
# =====================================================================
$ErrorActionPreference = 'Continue'
# fzip emits UTF-8; tell PowerShell to decode captured output correctly
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# Cleanup must survive what the security tests create: paths over 260
# characters and files named after reserved devices. Remove-Item chokes on
# both, so fall back to cmd's rmdir and then to a fresh unique folder.
$S = Join-Path $env:TEMP "fzip_tests"
if (Test-Path -LiteralPath $S) {
  try { Remove-Item -LiteralPath $S -Recurse -Force -Confirm:$false -ErrorAction Stop } catch {}
}
# The \\?\ prefix is what lets rmdir reach paths beyond 260 characters,
# which test C02 deliberately creates.
if (Test-Path -LiteralPath $S) { cmd /c "rmdir /s /q `"\\?\$S`"" 2>&1 | Out-Null }
if (Test-Path -LiteralPath $S) {
  $S = Join-Path $env:TEMP ("fzip_tests_" + [guid]::NewGuid().ToString('N').Substring(0,8))
  Write-Output "NOTE: previous test folder could not be removed; using $S"
}
New-Item -ItemType Directory -Force $S | Out-Null

$FZIP = Join-Path $PSScriptRoot "target\release\fzip.exe"
if (-not (Test-Path -LiteralPath $FZIP)) { $FZIP = Join-Path $PSScriptRoot "fzip.exe" }
$SEVENZ = "C:\Program Files\7-Zip\7z.exe"
$RAR    = "C:\Program Files\WinRAR\Rar.exe"
$hasSevenZ = Test-Path $SEVENZ
$hasRar    = Test-Path $RAR

Add-Type -AssemblyName System.IO.Compression.FileSystem
Add-Type -AssemblyName System.IO.Compression

$script:pass = 0; $script:fail = 0; $script:skip = 0; $script:failed = @()

function Assert($cond, $name) {
  if ($cond) { $script:pass++; Write-Output "PASS  $name" }
  else { $script:fail++; $script:failed += $name; Write-Output "FAIL  $name" }
}
function Skip($name, $why) { $script:skip++; Write-Output "SKIP  $name ($why)" }

function HashDir($dir) {
  if (-not (Test-Path -LiteralPath $dir)) { return "<missing>" }
  (Get-ChildItem -LiteralPath $dir -Recurse -File | ForEach-Object {
    "$($_.FullName.Substring($dir.Length))|$((Get-FileHash -LiteralPath $_.FullName -Algorithm MD5).Hash)"
  } | Sort-Object) -join "`n"
}

function NewZip($path, [hashtable]$entries, $level = 'Optimal') {
  $fs = [System.IO.File]::Create($path)
  $z = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
  foreach ($k in $entries.Keys) {
    $en = $z.CreateEntry($k, [System.IO.Compression.CompressionLevel]::$level)
    $st = $en.Open()
    $b = [System.Text.Encoding]::UTF8.GetBytes($entries[$k])
    $st.Write($b, 0, $b.Length); $st.Dispose()
  }
  $z.Dispose(); $fs.Dispose()
}

# ---------- source data ----------
# Non-ASCII names are built from code points: PowerShell 5.1 reads a BOM-less
# script as ANSI, which would corrupt literal non-ASCII text in this file.
$uniDir  = [string]::Concat([char]0x65E5,[char]0x672C,[char]0x8A9E,'_caf',[char]0xE9)   # "日本語_café"
$uniFile = [string]::Concat('h',[char]0xE0,'ng_',[char]0x4F60,[char]0x597D,'.txt')      # "hàng_你好.txt"

$data = "$S\data"
New-Item -ItemType Directory -Force "$data\sub\deep" | Out-Null
New-Item -ItemType Directory -Force "$data\$uniDir" | Out-Null
$rnd = New-Object System.Random(7)
for ($i = 0; $i -lt 40; $i++) {
  $sb = New-Object System.Text.StringBuilder
  $reps = $rnd.Next(100, 3000)
  for ($j = 0; $j -lt $reps; $j++) { [void]$sb.Append("line $i repeated many times so it compresses well ") }
  [System.IO.File]::WriteAllText("$data\sub\f$i.txt", $sb.ToString())
}
$bin = New-Object byte[] (8MB); $rnd.NextBytes($bin)
[System.IO.File]::WriteAllBytes("$data\sub\deep\random.bin", $bin)
[System.IO.File]::WriteAllText("$data\$uniDir\$uniFile", "unicode content")
[System.IO.File]::WriteAllText("$data\empty.txt", "")
$SRC = HashDir $data

Write-Output "===== A. ZIP basics ====="

[System.IO.Compression.ZipFile]::CreateFromDirectory($data, "$S\a1.zip", 'Optimal', $false, [System.Text.Encoding]::UTF8)
& $FZIP x "$S\a1.zip" -o "$S\o_a1" -q
Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_a1") -eq $SRC)) "A01 .NET zip: every file hash matches"

if ($hasSevenZ) {
  & $SEVENZ a -tzip "$S\a2.zip" "$data\*" -bso0 -bsp0 | Out-Null
  & $FZIP x "$S\a2.zip" -o "$S\o_a2" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_a2") -eq $SRC)) "A02 7-Zip zip: hashes match"
  & $SEVENZ a -tzip -mx=0 "$S\a3.zip" "$data\*" -bso0 -bsp0 | Out-Null
  & $FZIP x "$S\a3.zip" -o "$S\o_a3" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_a3") -eq $SRC)) "A03 stored (uncompressed) zip: hashes match"
} else { Skip "A02/A03 7-Zip zips" "7z.exe not installed" }

$z = [System.IO.Compression.ZipFile]::Open("$S\a4.zip", 'Create'); $z.Dispose()
& $FZIP x "$S\a4.zip" -o "$S\o_a4" -q
Assert ($LASTEXITCODE -eq 0) "A04 empty zip: no error"

$z = [System.IO.Compression.ZipFile]::Open("$S\a5.zip", 'Create')
[void]$z.CreateEntry("dir_a/"); [void]$z.CreateEntry("dir_b/nested/"); [void]$z.CreateEntry("blank.txt")
$z.Dispose()
& $FZIP x "$S\a5.zip" -o "$S\o_a5" -q
Assert (($LASTEXITCODE -eq 0) -and (Test-Path "$S\o_a5\dir_a") -and ((Get-Item "$S\o_a5\blank.txt").Length -eq 0)) "A05 empty folders and 0-byte file"

Write-Output "===== B. ZIP encryption ====="

if ($hasSevenZ) {
  foreach ($pair in @(@('AES256','b1'), @('AES128','b2'), @('AES192','b3'), @('ZipCrypto','b4'))) {
    $mem = $pair[0]; $tag = $pair[1]
    & $SEVENZ a -tzip "-pP@ss2026" "-mem=$mem" "$S\$tag.zip" "$data\sub" -bso0 -bsp0 | Out-Null
    & $FZIP x "$S\$tag.zip" -o "$S\o_$tag" -p "P@ss2026" -q
    Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_$tag\sub") -eq (HashDir "$data\sub"))) "B0$($tag.Substring(1)) $mem with correct password"
  }
  & $FZIP x "$S\b1.zip" -o "$S\o_b5" -p "wrongpass" -q 2>$null
  Assert ($LASTEXITCODE -ne 0) "B05 AES wrong password is rejected"
  & $FZIP x "$S\b4.zip" -o "$S\o_b6" -p "wrongpass" -q 2>$null
  Assert ($LASTEXITCODE -ne 0) "B06 ZipCrypto wrong password is rejected"
  $msg = & $FZIP x "$S\b1.zip" -o "$S\o_b7" -q 2>&1 | Out-String
  Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'password')) "B07 missing -p reports that a password is needed"
} else { Skip "B01-B07 encryption" "7z.exe not installed" }

Write-Output "===== C. Security (previously found bugs) ====="

# C01: Windows device names must be renamed, ordinary names left alone
NewZip "$S\c1.zip" @{ 'CON'='x'; 'PRN.txt'='x'; 'NUL'='x'; 'COM1'='x'; 'LPT1.dat'='x'; 'aux'='x'; 'ok.txt'='fine'; 'console.log'='normal' }
& $FZIP x "$S\c1.zip" -o "$S\o_c1" -q
$names = (Get-ChildItem "$S\o_c1" -File | ForEach-Object { $_.Name }) -join ','
$noDevice = -not ($names -split ',' | Where-Object { $_ -in @('CON','PRN.txt','NUL','COM1','LPT1.dat','aux') })
Assert ($noDevice -and ($names -match '_CON') -and ($names -match 'console\.log')) "C01 reserved device names are renamed safely"

# C02: a path over 260 characters must really be created (silent data loss fix)
$deep = ((1..25 | ForEach-Object { "averyverylongfoldername$_" }) -join '/') + "/final.txt"
NewZip "$S\c2.zip" @{ $deep = 'DEEPEST CONTENT'; 'shallow.txt' = 'ok' }
& $FZIP x "$S\c2.zip" -o "$S\o_c2" -q
$longPath = "\\?\$S\o_c2\" + ($deep -replace '/','\')
$exists = [System.IO.File]::Exists($longPath)
$content = if ($exists) { [System.IO.File]::ReadAllText($longPath) } else { "" }
Assert (($LASTEXITCODE -eq 0) -and $exists -and ($content -eq 'DEEPEST CONTENT')) "C02 path over 260 chars: file really exists (data-loss bug fixed)"

# C03: zip-slip blocked, ordinary entry still extracted
$fs = [System.IO.File]::Create("$S\c3.zip")
$z = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
foreach ($n in @('../evil_escape.txt','..\..\evil2.txt','harmless.txt')) {
  $e = $z.CreateEntry($n); $w = New-Object System.IO.StreamWriter($e.Open()); $w.Write("X"); $w.Dispose()
}
$z.Dispose(); $fs.Dispose()
New-Item -ItemType Directory -Force "$S\o_c3\inner" | Out-Null
$out = & $FZIP x "$S\c3.zip" -o "$S\o_c3\inner" 2>&1 | Out-String
$escaped = (Test-Path "$S\o_c3\evil_escape.txt") -or (Test-Path "$S\evil2.txt")
Assert ((-not $escaped) -and (Test-Path "$S\o_c3\inner\harmless.txt")) "C03 zip-slip is blocked"

# C04: skipped entries must be reported (silent-skip bug fixed)
Assert ($out -match 'SKIPPED|unsafe') "C04 unsafe entries are reported, not dropped silently"

# C05: peak RAM must stay flat for a large member (streaming fix)
$big = New-Object byte[] (200MB)
for ($k = 0; $k -lt $big.Length; $k += 4096) { $big[$k] = [byte]($k % 251) }
[System.IO.File]::WriteAllBytes("$S\bigfile.bin", $big)
Remove-Variable big
& $FZIP a "$S\c5.zip" "$S\bigfile.bin" -y -q -mx1
$p = Start-Process -FilePath $FZIP -ArgumentList "x","`"$S\c5.zip`"","-o","`"$S\o_c5`"","-q" -PassThru -NoNewWindow
$peak = 0
while (-not $p.HasExited) {
  Start-Sleep -Milliseconds 60
  try { $p.Refresh(); if ($p.WorkingSet64 -gt $peak) { $peak = $p.WorkingSet64 } } catch {}
}
$peakMB = [math]::Round($peak/1MB,1)
$ok = ([System.IO.File]::Exists("$S\o_c5\bigfile.bin")) -and ($peak -lt 160MB)
Assert $ok "C05 200MB member: peak RAM $peakMB MB (streams rather than buffering)"
Remove-Item "$S\bigfile.bin" -Force -ErrorAction SilentlyContinue

# C06: corrupt archive caught by CRC
Copy-Item "$S\a1.zip" "$S\c6.zip"
$b = [System.IO.File]::ReadAllBytes("$S\c6.zip"); $m = [int]($b.Length*0.4); $b[$m] = $b[$m] -bxor 0xFF
[System.IO.File]::WriteAllBytes("$S\c6.zip", $b)
& $FZIP x "$S\c6.zip" -o "$S\o_c6" -q 2>$null
Assert ($LASTEXITCODE -ne 0) "C06 one flipped byte is caught by CRC"

# C07: file that only pretends to be a zip
[System.IO.File]::WriteAllText("$S\c7.zip", "this is definitely not a zip file")
$msg = & $FZIP x "$S\c7.zip" -o "$S\o_c7" -q 2>&1 | Out-String
Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'valid zip')) "C07 fake .zip gives a clear error"

Write-Output "===== D. Formats this version does not read ====="

# Fzip 2.x reads zip only. Anyone arriving from 1.x with a .rar deserves to be
# told which format it is, not left with a parse error about a missing record.
if ($hasRar) {
  Push-Location $data
  & $RAR a -r -idq "$S\d1.rar" "sub" | Out-Null
  Pop-Location
  $msg = & $FZIP x "$S\d1.rar" -o "$S\o_d1" -q 2>&1 | Out-String
  Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'RAR archive') -and ($msg -match 'zip only')) `
         "D01 a RAR file is named as such, not reported as a broken zip"
} else { Skip "D01 RAR message" "Rar.exe not installed" }

if ($hasSevenZ) {
  Push-Location $data
  & $SEVENZ a "$S\d2.7z" "sub" -bso0 -bsp0 | Out-Null
  & $SEVENZ a "$S\d3.tar" "sub" -bso0 -bsp0 | Out-Null
  Pop-Location
  & $SEVENZ a -tgzip "$S\d4.txt.gz" "$data\sub\f0.txt" -bso0 -bsp0 | Out-Null

  $msg = & $FZIP x "$S\d2.7z" -o "$S\o_d2" -q 2>&1 | Out-String
  Assert (($LASTEXITCODE -ne 0) -and ($msg -match '7z archive')) "D02 a 7z file is named as such"
  $msg = & $FZIP x "$S\d3.tar" -o "$S\o_d3" -q 2>&1 | Out-String
  Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'tar archive')) "D03 a tar file is named as such"
  $msg = & $FZIP x "$S\d4.txt.gz" -o "$S\o_d4" -q 2>&1 | Out-String
  Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'gzip file')) "D04 a gzip file is named as such"
} else { Skip "D02-D04 foreign format messages" "7z.exe not installed" }

Write-Output "===== F. Compression (the 'a' command) ====="

& $FZIP a "$S\f1.zip" "$data" -y -q
Assert (($LASTEXITCODE -eq 0) -and (Test-Path "$S\f1.zip")) "F01 create zip: succeeds"

& $FZIP x "$S\f1.zip" -o "$S\o_f1" -q
Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_f1\data") -eq $SRC)) "F02 fzip compress then extract: round trip is lossless"

if ($hasSevenZ) {
  & $SEVENZ t "$S\f1.zip" -bso0 -bsp0 | Out-Null
  Assert ($LASTEXITCODE -eq 0) "F03 zip written by fzip: 7-Zip confirms it is VALID"
  & $SEVENZ x "$S\f1.zip" "-o$S\o_f3" -y -bso0 -bsp0 | Out-Null
  Assert ((HashDir "$S\o_f3\data") -eq $SRC) "F04 zip written by fzip: 7-Zip extracts identical bytes"
}

# .NET is the other reader almost every Windows machine has, and it is stricter
# than 7-Zip about the central directory.
$netOk = $false
try {
  $za = [System.IO.Compression.ZipFile]::OpenRead("$S\f1.zip")
  $netOk = $za.Entries.Count -gt 0
  $za.Dispose()
} catch { $netOk = $false }
Assert $netOk "F04b zip written by fzip: .NET ZipArchive reads it"

& $FZIP a "$S\f5.zip" "$data" -y -q -p "P@ss2026"
& $FZIP x "$S\f5.zip" -o "$S\o_f5" -p "P@ss2026" -q
Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_f5\data") -eq $SRC)) "F05 AES-256 round trip: hashes match"

& $FZIP x "$S\f5.zip" -o "$S\o_f6" -p "wrong" -q 2>$null
Assert ($LASTEXITCODE -ne 0) "F06 AES zip from fzip: wrong password rejected"

if ($hasSevenZ) {
  & $SEVENZ t "$S\f5.zip" "-pP@ss2026" -bso0 -bsp0 | Out-Null
  Assert ($LASTEXITCODE -eq 0) "F07 AES-256 zip from fzip: 7-Zip can read it (interoperable)"
}

& $FZIP a "$S\f8.zip" "$data" -y -q -mx0
& $FZIP a "$S\f9.zip" "$data" -y -q -mx9
$s0 = (Get-Item "$S\f8.zip").Length; $s9 = (Get-Item "$S\f9.zip").Length
Assert ($s9 -lt $s0) "F08 levels honoured: -mx9 ($([math]::Round($s9/1KB))KB) < -mx0 ($([math]::Round($s0/1KB))KB)"

& $FZIP a "$S\f10.zip" "$data" -y -q -x "*.txt"
$l = & $FZIP l "$S\f10.zip" | Out-String
Assert (($l -notmatch '\.txt') -and ($l -match 'random\.bin')) "F09 create with -x: excludes the right files"

# 7-Zip-style attached password, e.g. -pSECRET
& $FZIP a "$S\f11.zip" "$data\sub" -y -q -pATTACHED123
& $FZIP x "$S\f11.zip" -o "$S\o_f11" -pATTACHED123 -q
$rt = ($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_f11\sub") -eq (HashDir "$data\sub"))
& $FZIP x "$S\f11.zip" -o "$S\o_f11bad" -pWRONGPASS -q 2>$null
Assert ($rt -and ($LASTEXITCODE -ne 0)) "F10 -pSECRET attached form: round trip works, wrong password rejected"

# A bare -p cannot prompt in a script. It must fail loudly rather than
# quietly writing an UNENCRYPTED archive.
$msg = & $FZIP a "$S\f12.zip" "$data\sub" -y -q -p 2>&1 | Out-String
Assert (($LASTEXITCODE -eq 7) -and ($msg -match 'not an interactive terminal') -and (-not (Test-Path "$S\f12.zip"))) "F11 bare -p without a terminal: errors out, writes no unencrypted archive"

Write-Output "===== G. Commands and options ====="

& $FZIP t "$S\a1.zip" -q
Assert ($LASTEXITCODE -eq 0) "G01 't' on a good zip returns 0"
& $FZIP t "$S\c6.zip" -q 2>$null
Assert ($LASTEXITCODE -ne 0) "G02 't' on a corrupt zip reports failure"
$before = (Get-ChildItem $S -Directory).Count
& $FZIP t "$S\a1.zip" -q
Assert ((Get-ChildItem $S -Directory).Count -eq $before) "G03 't' writes nothing to disk"

& $FZIP x "$S\a1.zip" -o "$S\o_g4" -i "*.bin" -q
$g4 = Get-ChildItem "$S\o_g4" -Recurse -File
Assert (($g4.Count -eq 1) -and ($g4[0].Name -eq 'random.bin')) "G04 -i extracts only matching names"

& $FZIP x "$S\a1.zip" -o "$S\o_g5" -x "*.txt" -q
$g5 = Get-ChildItem "$S\o_g5" -Recurse -File
Assert (($g5.Count -gt 0) -and -not ($g5 | Where-Object { $_.Extension -eq '.txt' })) "G05 -x excludes matching names"

& $FZIP x "$S\a1.zip" -o "$S\o_g6" -e -q
Assert ((Get-ChildItem "$S\o_g6" -Directory).Count -eq 0) "G06 -e flattens the folder structure"

New-Item -ItemType Directory -Force "$S\o_g7" | Out-Null
[System.IO.File]::WriteAllText("$S\o_g7\empty.txt", "KEEP ME")
& $FZIP x "$S\a1.zip" -o "$S\o_g7" --overwrite skip -q 2>$null
Assert ([System.IO.File]::ReadAllText("$S\o_g7\empty.txt") -eq 'KEEP ME') "G07 --overwrite skip leaves existing files alone"

New-Item -ItemType Directory -Force "$S\o_g8" | Out-Null
[System.IO.File]::WriteAllText("$S\o_g8\empty.txt", "ORIGINAL")
& $FZIP x "$S\a1.zip" -o "$S\o_g8" --overwrite rename -q
Assert ((Test-Path "$S\o_g8\empty_1.txt") -and ([System.IO.File]::ReadAllText("$S\o_g8\empty.txt") -eq 'ORIGINAL')) "G08 --overwrite rename writes a _1 copy"

$v = & $FZIP -V | Out-String
Assert ($v -match 'fzip \d+\.\d+\.\d+') "G09 -V prints the version"

$h = & $FZIP -h | Out-String
Assert (($h -match 'COMMANDS') -and ($h -match 'EXIT CODES')) "G10 -h prints usage and exit codes"

$msg = & $FZIP x "$S\does_not_exist.zip" 2>&1 | Out-String
Assert (($LASTEXITCODE -eq 7) -and ($msg -match 'not found')) "G11 missing file: exit 7 (7-Zip convention)"

$out = & $FZIP x "$S\a1.zip" -o "$S\o_g12" --progress 2>&1 | Out-String
Assert (($out -match '100\.0%') -and ($out -match 'ETA')) "G12 --progress shows a bar with ETA"

# Only the program's own text is checked; extracted file names legitimately
# contain non-ASCII characters supplied by the test data.
$uiText = ($h + (& $FZIP -V | Out-String) + (& $FZIP x "$S\does_not_exist.zip" 2>&1 | Out-String))
Assert (-not ($uiText -match '[^\x00-\x7F]')) "G13 interface text is pure ASCII English"

# G14: with no -o, the output folder must be created NEXT TO THE ARCHIVE, not in
# the current directory. This is what makes drag-and-drop onto fzip.exe usable:
# Explorer sets the working directory to fzip.exe's own folder.
New-Item -ItemType Directory -Force "$S\g14_store","$S\g14_cwd" | Out-Null
Copy-Item "$S\a1.zip" "$S\g14_store\payload.zip"
Push-Location "$S\g14_cwd"
& $FZIP "$S\g14_store\payload.zip" -q
Pop-Location
Assert ((Test-Path "$S\g14_store\payload") -and ((Get-ChildItem "$S\g14_cwd").Count -eq 0)) `
       "G14 default output lands beside the archive, not in the working directory"

Write-Output "===== H. Format detection and ZIP64 ====="

Copy-Item "$S\a1.zip" "$S\h1.rar"
& $FZIP x "$S\h1.rar" -o "$S\o_h1" -q
Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_h1") -eq $SRC)) "H01 zip named .rar: detected by magic bytes, extracted anyway"

# ZIP64: more than 65535 entries
$fs = [System.IO.File]::Create("$S\h4.zip")
$z = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
for ($i = 0; $i -lt 70000; $i++) {
  $e = $z.CreateEntry("d$($i % 100)/f$i.txt", [System.IO.Compression.CompressionLevel]::NoCompression)
  $st = $e.Open(); $b = [System.Text.Encoding]::UTF8.GetBytes("x$i"); $st.Write($b,0,$b.Length); $st.Dispose()
}
$z.Dispose(); $fs.Dispose()
$lst = & $FZIP l "$S\h4.zip" | Select-Object -Last 1
Assert ($lst -match '70000') "H04 ZIP64 with 70000 entries: all listed"

Write-Output "===== I. Hostile input (regressions for audited defects) ====="

# PowerShell 5.1 parses hex literals as the smallest SIGNED type, so
# 0xFFFFFFFF becomes -1. Build unsigned values through Convert instead.
function W16($ms,$v){ $ms.Write([BitConverter]::GetBytes([uint16]$v),0,2) }
function W32($ms,$v){ $ms.Write([BitConverter]::GetBytes([uint32]$v),0,4) }
function W64($ms,$v){ $ms.Write([BitConverter]::GetBytes([uint64]$v),0,8) }

# I01: a ZIP64 end record pointing far outside the file. Adding to that offset
# must not wrap past the bounds check and index the mapping out of range.
$ms = New-Object System.IO.MemoryStream
$U64 = [Convert]::ToUInt64("FFFFFFFFFFFFFFF0", 16)
$U32 = [uint32]::MaxValue
W32 $ms 0x06064b50; W64 $ms 44; W16 $ms 45; W16 $ms 45; W32 $ms 0; W32 $ms 0
W64 $ms 1; W64 $ms 1; W64 $ms 100; W64 $ms $U64
W32 $ms 0x07064b50; W32 $ms 0; W64 $ms 0; W32 $ms 1
W32 $ms 0x06054b50; W16 $ms 0; W16 $ms 0; W16 $ms 0xFFFF; W16 $ms 0xFFFF
W32 $ms 100; W32 $ms $U32; W16 $ms 0
[System.IO.File]::WriteAllBytes("$S\i1.zip", $ms.ToArray()); $ms.Dispose()
$msg = & $FZIP l "$S\i1.zip" 2>&1 | Out-String
Assert (($LASTEXITCODE -in @(1,2,7)) -and ($msg -notmatch 'panic|goroutine')) `
       "I01 hostile ZIP64 offset: clean error, no panic"

# I02: the same trick via a central-directory local-header offset.
$fs = [System.IO.File]::Create("$S\i2seed.zip")
$z = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
$e = $z.CreateEntry("a.txt"); $w = New-Object System.IO.StreamWriter($e.Open()); $w.Write("hi"); $w.Dispose()
$z.Dispose(); $fs.Dispose()
$bb = [System.IO.File]::ReadAllBytes("$S\i2seed.zip")
for ($i = 0; $i -lt $bb.Length-4; $i++) {
  if ($bb[$i] -eq 0x50 -and $bb[$i+1] -eq 0x4B -and $bb[$i+2] -eq 0x01 -and $bb[$i+3] -eq 0x02) {
    $bb[$i+42]=0xFF; $bb[$i+43]=0xFF; $bb[$i+44]=0xFF; $bb[$i+45]=0xFF; break
  }
}
[System.IO.File]::WriteAllBytes("$S\i2.zip", $bb)
$msg = & $FZIP t "$S\i2.zip" 2>&1 | Out-String
Assert (($LASTEXITCODE -in @(1,2,7)) -and ($msg -notmatch 'panic|goroutine')) `
       "I02 hostile local-header offset: clean error, no panic"

# I03: a deflate zip bomb. Only the declared size bounds the output, and a
# hostile archive simply understates it, so the decoder must be capped
# independently of what the entry claims.
$fs = [System.IO.File]::Create("$S\i3.zip")
$z = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
$en = $z.CreateEntry("zeros.bin", [System.IO.Compression.CompressionLevel]::Optimal)
$st = $en.Open()
$chunk = New-Object byte[] (4MB)
for ($i = 0; $i -lt 50; $i++) { $st.Write($chunk, 0, $chunk.Length) }
$st.Dispose(); $z.Dispose(); $fs.Dispose()
# Understate the uncompressed size as 1024 bytes in both headers
$bb = [System.IO.File]::ReadAllBytes("$S\i3.zip")
for ($i = 0; $i -lt $bb.Length-4; $i++) {
  if ($bb[$i] -eq 0x50 -and $bb[$i+1] -eq 0x4B -and $bb[$i+2] -eq 0x01 -and $bb[$i+3] -eq 0x02) {
    $bb[$i+24]=0x00; $bb[$i+25]=0x04; $bb[$i+26]=0x00; $bb[$i+27]=0x00
  }
  if ($bb[$i] -eq 0x50 -and $bb[$i+1] -eq 0x4B -and $bb[$i+2] -eq 0x03 -and $bb[$i+3] -eq 0x04) {
    $bb[$i+22]=0x00; $bb[$i+23]=0x04; $bb[$i+24]=0x00; $bb[$i+25]=0x00
  }
}
[System.IO.File]::WriteAllBytes("$S\i3.zip", $bb)
$kb = [math]::Round((Get-Item "$S\i3.zip").Length/1KB,1)
$msg = & $FZIP t "$S\i3.zip" 2>&1 | Out-String
Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'declared size')) `
       "I03 deflate bomb ($kb KB claiming 1 KB, really 200 MB): refused"

# I03b: the same bomb, but sized to take the STREAMING path instead.
#
# An entry is streamed to disk once its declared size passes 32 MB, and until
# 1.3.0 that path had no cap at all: only the buffered path checked. An entry
# declaring 40 MB and expanding to 200 MB was therefore written to disk in full,
# and only reported afterwards - by which point the damage is done. The limit now
# lives where every byte passes through, and is checked BEFORE the write.
$fs = [System.IO.File]::Create("$S\i3s.zip")
$z = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
$en = $z.CreateEntry("stream_bomb.bin", [System.IO.Compression.CompressionLevel]::Optimal)
$st = $en.Open()
$chunk = New-Object byte[] (4MB)
for ($i = 0; $i -lt 50; $i++) { $st.Write($chunk, 0, $chunk.Length) }   # 200 MB
$st.Dispose(); $z.Dispose(); $fs.Dispose()
# Declare 40 MB (0x02800000): over the 32 MB streaming threshold, far under the truth
$bb = [System.IO.File]::ReadAllBytes("$S\i3s.zip")
for ($i = 0; $i -lt $bb.Length-4; $i++) {
  if ($bb[$i] -eq 0x50 -and $bb[$i+1] -eq 0x4B -and $bb[$i+2] -eq 0x01 -and $bb[$i+3] -eq 0x02) {
    $bb[$i+24]=0x00; $bb[$i+25]=0x00; $bb[$i+26]=0x80; $bb[$i+27]=0x02
  }
  if ($bb[$i] -eq 0x50 -and $bb[$i+1] -eq 0x4B -and $bb[$i+2] -eq 0x03 -and $bb[$i+3] -eq 0x04) {
    $bb[$i+22]=0x00; $bb[$i+23]=0x00; $bb[$i+24]=0x80; $bb[$i+25]=0x02
  }
}
[System.IO.File]::WriteAllBytes("$S\i3s.zip", $bb)
$msg = & $FZIP x "$S\i3s.zip" -o "$S\o_i3s" 2>&1 | Out-String
$written = if (Test-Path "$S\o_i3s\stream_bomb.bin") { (Get-Item "$S\o_i3s\stream_bomb.bin").Length } else { 0 }
Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'declared size') -and ($written -le 40MB)) `
       "I03b streaming bomb (declares 40 MB, holds 200 MB): refused, $([math]::Round($written/1MB))MB reached disk"

# I04: exactly 65535 entries. 0xFFFF is the ZIP64 sentinel, so this count must
# emit a ZIP64 record; a `>` comparison here produced archives fzip could not
# read back. Slow by nature - it really does create 65534 files.
$many = "$S\many"
New-Item -ItemType Directory -Force $many | Out-Null
for ($i = 0; $i -lt 65534; $i++) { [System.IO.File]::WriteAllText("$many\f$i", "") }
& $FZIP a "$S\i4.zip" $many -y -q -mx0
$readback = & $FZIP l "$S\i4.zip" 2>&1 | Out-String
$sevenOk = $true
if ($hasSevenZ) { & $SEVENZ t "$S\i4.zip" -bso0 -bsp0 | Out-Null; $sevenOk = ($LASTEXITCODE -eq 0) }
Assert (($readback -notmatch 'corrupt') -and ($readback -match '65534 files') -and $sevenOk) `
       "I04 exactly 65535 entries: ZIP64 emitted, fzip and 7-Zip both read it"

# I05: a large member must stream through compression and encryption, not
# buffer. Before this was fixed the path held the file plus three transformed
# copies in RAM at once.
$encSrc = "$S\encsrc"
New-Item -ItemType Directory -Force $encSrc | Out-Null
$big = New-Object byte[] (150MB)
for ($k = 0; $k -lt $big.Length; $k += 2048) { $big[$k] = [byte]($k % 251) }
[System.IO.File]::WriteAllBytes("$encSrc\huge.bin", $big); Remove-Variable big
$p = Start-Process -FilePath $FZIP -ArgumentList "a","`"$S\i5.zip`"","`"$encSrc`"","-y","-q","-p","P@ss2026" `
     -PassThru -NoNewWindow
$peak = 0
while (-not $p.HasExited) { Start-Sleep -Milliseconds 50; try { $p.Refresh(); if ($p.WorkingSet64 -gt $peak) { $peak=$p.WorkingSet64 } } catch {} }
$peakMB = [math]::Round($peak/1MB,1)
& $FZIP x "$S\i5.zip" -o "$S\o_i5" -p "P@ss2026" -q
$same = ((Get-FileHash "$encSrc\huge.bin" -Algorithm MD5).Hash -eq
         (Get-FileHash "$S\o_i5\encsrc\huge.bin" -Algorithm MD5).Hash)
Assert (($LASTEXITCODE -eq 0) -and $same -and ($peak -lt 160MB)) `
       "I05 150MB encrypted member: writes at $peakMB MB peak, round-trips intact"

Write-Output "===== J. Field reports (regressions from real-world use) ====="

# .NET's ZipArchive rewrites '\' to '/', so these need a zip built by hand.
# Stored entries only, CRC left at zero - the tests below pass --no-crc.
function New-RawZip($Path, [hashtable]$Entries, [bool]$SetDirAttr = $true) {
  $ms = New-Object System.IO.MemoryStream
  $bw = New-Object System.IO.BinaryWriter($ms)
  $cd = New-Object System.Collections.ArrayList
  foreach ($k in $Entries.Keys) {
    $n = [System.Text.Encoding]::UTF8.GetBytes($k)
    $d = [System.Text.Encoding]::UTF8.GetBytes([string]$Entries[$k])
    $off = $ms.Position
    $bw.Write([uint32]0x04034b50); $bw.Write([uint16]20); $bw.Write([uint16]0x0800)
    $bw.Write([uint16]0); $bw.Write([uint16]0); $bw.Write([uint16]0)
    $bw.Write([uint32]0); $bw.Write([uint32]$d.Length); $bw.Write([uint32]$d.Length)
    $bw.Write([uint16]$n.Length); $bw.Write([uint16]0); $bw.Write($n)
    if ($d.Length) { $bw.Write($d) }
    [void]$cd.Add(@{ n=$n; len=$d.Length; off=$off; k=$k })
  }
  $cdStart = $ms.Position
  foreach ($e in $cd) {
    $isDir = $e.k -match '[\\/]$'
    $attr = if ($isDir -and $SetDirAttr) { 0x10 } else { 0x20 }
    $bw.Write([uint32]0x02014b50); $bw.Write([uint16]20); $bw.Write([uint16]20)
    $bw.Write([uint16]0x0800); $bw.Write([uint16]0); $bw.Write([uint16]0); $bw.Write([uint16]0)
    $bw.Write([uint32]0); $bw.Write([uint32]$e.len); $bw.Write([uint32]$e.len)
    $bw.Write([uint16]$e.n.Length); $bw.Write([uint16]0); $bw.Write([uint16]0)
    $bw.Write([uint16]0); $bw.Write([uint16]0); $bw.Write([uint32]$attr)
    $bw.Write([uint32]$e.off); $bw.Write($e.n)
  }
  $cdSize = $ms.Position - $cdStart
  $bw.Write([uint32]0x06054b50); $bw.Write([uint16]0); $bw.Write([uint16]0)
  $bw.Write([uint16]$cd.Count); $bw.Write([uint16]$cd.Count)
  $bw.Write([uint32]$cdSize); $bw.Write([uint32]$cdStart); $bw.Write([uint16]0)
  $bw.Flush(); [System.IO.File]::WriteAllBytes($Path, $ms.ToArray())
  $bw.Dispose(); $ms.Dispose()
}

# J01: directory entries written with a trailing backslash. The spec says '/',
# but Windows producers write '\'. Treating one as a file made fzip try to
# create a file over an existing directory: "Access is denied".
# No DOS directory attribute here, so only the name can identify it.
New-RawZip "$S\j1.zip" @{ 'app\' = ''; 'app\lib\' = ''; 'app\lib\mod.txt' = 'CONTENT' } $false
& $FZIP x "$S\j1.zip" -o "$S\o_j1" -q --no-crc
Assert (($LASTEXITCODE -eq 0) -and
        (Test-Path "$S\o_j1\app\lib" -PathType Container) -and
        ([System.IO.File]::ReadAllText("$S\o_j1\app\lib\mod.txt") -eq 'CONTENT')) `
       "J01 directory entries ending in backslash are created as folders"

# J02: a hidden run must not stop at "Press Enter to exit". An installer gets a
# console allocated even with no window, so owning one is not evidence that
# anyone is watching.
$hidden = Start-Process -FilePath $FZIP `
          -ArgumentList "x","`"$S\a1.zip`"","-o","`"$S\o_j2`"","-q" `
          -WindowStyle Hidden -PassThru
$exited = $hidden.WaitForExit(30000)
if (-not $exited) { try { $hidden.Kill() } catch {} }
Assert ($exited -and $hidden.ExitCode -eq 0) "J02 hidden run exits instead of waiting for a keypress"

# J03: --no-pause must be honoured by a run that owns a VISIBLE console, which
# is the only situation where fzip would otherwise wait for a keypress. Sharing
# the caller's console (-NoNewWindow) would prove nothing: fzip never pauses
# there, because it does not own the console.
$np = Start-Process -FilePath $FZIP `
      -ArgumentList "x","`"$S\a1.zip`"","-o","`"$S\o_j3`"","-q","--no-pause" `
      -PassThru
$npOk = $np.WaitForExit(30000)
if (-not $npOk) { try { $np.Kill() } catch {} }
Assert ($npOk -and $np.ExitCode -eq 0) "J03 --no-pause exits cleanly from its own console"

# J03b: the counterpart. Without the flag, the same run must still be sitting at
# the prompt, which is what proves J03 tested something. Extraction itself takes
# well under a second, so three seconds is not a race.
$pz = Start-Process -FilePath $FZIP `
      -ArgumentList "x","`"$S\a1.zip`"","-o","`"$S\o_j3b`"","-q" `
      -PassThru
$exitedEarly = $pz.WaitForExit(3000)
if (-not $exitedEarly) { try { $pz.Kill() } catch {} }
Assert (-not $exitedEarly) "J03b without --no-pause a console-owning run waits, as intended"

# J03c: FZIP_NO_PAUSE is the environment-variable equivalent, for callers that
# cannot change the command line.
$env:FZIP_NO_PAUSE = "1"
$pe = Start-Process -FilePath $FZIP `
      -ArgumentList "x","`"$S\a1.zip`"","-o","`"$S\o_j3c`"","-q" `
      -PassThru
$peOk = $pe.WaitForExit(30000)
if (-not $peOk) { try { $pe.Kill() } catch {} }
Remove-Item Env:FZIP_NO_PAUSE
Assert ($peOk -and $pe.ExitCode -eq 0) "J03c FZIP_NO_PAUSE also suppresses the pause"

# J04: cmd.exe and PowerShell do not expand wildcards, so fzip has to.
$wild = "$S\wild"
New-Item -ItemType Directory -Force "$wild\deeper" | Out-Null
[System.IO.File]::WriteAllText("$wild\one.txt", "1")
[System.IO.File]::WriteAllText("$wild\two.log", "2")
[System.IO.File]::WriteAllText("$wild\deeper\three.txt", "3")
& $FZIP a "$S\j4.zip" "$wild\*" -y -q
$j4 = & $FZIP l "$S\j4.zip" | Out-String
Assert (($LASTEXITCODE -eq 0) -and ($j4 -match 'one\.txt') -and ($j4 -match 'three\.txt')) `
       "J04 wildcard input path is expanded, including nested folders"

$msg = & $FZIP a "$S\j5.zip" "$wild\*.nothing" -y -q 2>&1 | Out-String
Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'matched nothing')) `
       "J05 a wildcard matching nothing is reported"

# J06/J07: installing over a previous version means overwriting read-only files.
NewZip "$S\j6.zip" @{ 'locked.txt' = 'NEWVERSION' }
New-Item -ItemType Directory -Force "$S\o_j6" | Out-Null
[System.IO.File]::WriteAllText("$S\o_j6\locked.txt", "OLDVERSION")
Set-ItemProperty "$S\o_j6\locked.txt" -Name IsReadOnly -Value $true
& $FZIP x "$S\j6.zip" -o "$S\o_j6" --overwrite all -q
$j6 = [System.IO.File]::ReadAllText("$S\o_j6\locked.txt")
Assert (($LASTEXITCODE -eq 0) -and ($j6 -eq "NEWVERSION")) `
       "J06 --overwrite all replaces a read-only file"

New-Item -ItemType Directory -Force "$S\o_j7" | Out-Null
[System.IO.File]::WriteAllText("$S\o_j7\locked.txt", "KEEPME")
Set-ItemProperty "$S\o_j7\locked.txt" -Name IsReadOnly -Value $true
& $FZIP x "$S\j6.zip" -o "$S\o_j7" --overwrite skip -q 2>$null
$j7 = [System.IO.File]::ReadAllText("$S\o_j7\locked.txt")
Set-ItemProperty "$S\o_j7\locked.txt" -Name IsReadOnly -Value $false
Assert ($j7 -eq "KEEPME") "J07 --overwrite skip still leaves a read-only file alone"

# J08: a file entry colliding with an existing folder must be explained,
# rather than reported as a bare permissions error
New-RawZip "$S\j8.zip" @{ 'collide' = 'data' }
New-Item -ItemType Directory -Force "$S\o_j8\collide" | Out-Null
$msg8 = & $FZIP x "$S\j8.zip" -o "$S\o_j8" --no-crc 2>&1 | Out-String
Assert ($msg8 -match 'folder of that name already exists') `
       "J08 file-over-folder collision gives a readable message"

Write-Output ""
Write-Output "================================================================"
Write-Output "TOTAL: $script:pass passed, $script:fail failed, $script:skip skipped"
if ($script:fail -gt 0) { $script:failed | ForEach-Object { Write-Output "  FAILED: $_" } }
Write-Output "================================================================"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }
