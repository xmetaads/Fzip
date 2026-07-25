# =====================================================================
#  fzip - full test suite
#  Usage:  .\run_tests.ps1
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
if ($hasSevenZ) {
  & $SEVENZ a -tzip -mx=1 "$S\c5.zip" "$S\bigfile.bin" -bso0 -bsp0 | Out-Null
  $p = Start-Process -FilePath $FZIP -ArgumentList "x","`"$S\c5.zip`"","-o","`"$S\o_c5`"","-q" -PassThru -NoNewWindow
  $peak = 0
  while (-not $p.HasExited) {
    Start-Sleep -Milliseconds 60
    try { $p.Refresh(); if ($p.WorkingSet64 -gt $peak) { $peak = $p.WorkingSet64 } } catch {}
  }
  $peakMB = [math]::Round($peak/1MB,1)
  $ok = ([System.IO.File]::Exists("$S\o_c5\bigfile.bin")) -and ($peak -lt 120MB)
  Assert $ok "C05 200MB member: peak RAM $peakMB MB (streams; was 233 MB before)"
} else { Skip "C05 memory ceiling" "7z.exe not installed" }
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
Assert (($LASTEXITCODE -ne 0) -and ($msg -match 'valid ZIP')) "C07 fake .zip gives a clear error"

Write-Output "===== D. Additional formats (7z, tar, gz, bz2, xz) ====="

if ($hasSevenZ) {
  Push-Location $data
  & $SEVENZ a "$S\d.7z" "*" -bso0 -bsp0 | Out-Null
  & $SEVENZ a "$S\d.tar" "*" -bso0 -bsp0 | Out-Null
  Pop-Location
  & $SEVENZ a "$S\d.tar.gz"  "$S\d.tar" -bso0 -bsp0 | Out-Null
  & $SEVENZ a "$S\d.tar.bz2" "$S\d.tar" -bso0 -bsp0 | Out-Null
  & $SEVENZ a "$S\d.tar.xz"  "$S\d.tar" -bso0 -bsp0 | Out-Null

  & $FZIP x "$S\d.7z" -o "$S\o_d7z" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_d7z") -eq $SRC)) "D01 7z: extracts, hashes match"

  & $FZIP x "$S\d.tar" -o "$S\o_dtar" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_dtar") -eq $SRC)) "D02 tar: extracts, hashes match"

  # fzip spots the tar inside and unpacks its contents directly, like `tar -xzf`,
  # whereas 7-Zip needs two passes.
  foreach ($pair in @(@('gz','d.tar.gz'), @('bz2','d.tar.bz2'), @('xz','d.tar.xz'))) {
    $ext = $pair[0]; $f = $pair[1]
    & $FZIP x "$S\$f" -o "$S\o_d$ext" -q
    Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_d$ext") -eq $SRC)) "D03-${ext}: tar.$ext unpacks contents in one step"
  }

  & $SEVENZ a "-pP@ss2026" "$S\d_enc.7z" "$data\sub" -bso0 -bsp0 | Out-Null
  & $FZIP x "$S\d_enc.7z" -o "$S\o_d7zp" -p "P@ss2026" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_d7zp\sub") -eq (HashDir "$data\sub"))) "D04 encrypted 7z: hashes match"
  & $FZIP x "$S\d_enc.7z" -o "$S\o_d7zbad" -p "wrong" -q 2>$null
  Assert ($LASTEXITCODE -ne 0) "D05 encrypted 7z: wrong password rejected"

  & $SEVENZ a -tgzip "$S\single.txt.gz" "$data\sub\f0.txt" -bso0 -bsp0 | Out-Null
  & $FZIP x "$S\single.txt.gz" -o "$S\o_dsingle" -q
  Assert (($LASTEXITCODE -eq 0) -and (Test-Path "$S\o_dsingle\single.txt")) "D06 plain gzip file: correct output name"
} else { Skip "D01-D06 extra formats" "7z.exe not installed" }

Write-Output "===== E. RAR ====="

if ($hasRar) {
  Push-Location $data
  & $RAR a -r -idq "$S\e1.rar" "sub" "$uniDir" "empty.txt" | Out-Null
  & $RAR a -r -s -idq "$S\e2.rar" "sub" "$uniDir" "empty.txt" | Out-Null
  & $RAR a -r -v300k -idq "$S\e3vol.rar" "sub" "$uniDir" "empty.txt" | Out-Null
  & $RAR a -r "-pP@ss2026" -idq "$S\e4.rar" "sub" | Out-Null
  & $RAR a -r "-hpP@ss2026" -idq "$S\e5.rar" "sub" | Out-Null
  Pop-Location

  & $FZIP x "$S\e1.rar" -o "$S\o_e1" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_e1") -eq $SRC)) "E01 RAR5: hashes match"
  & $FZIP x "$S\e2.rar" -o "$S\o_e2" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_e2") -eq $SRC)) "E02 solid RAR: hashes match"
  $v1 = Get-ChildItem "$S\e3vol*.rar" | Sort-Object Name | Select-Object -First 1
  & $FZIP x $v1.FullName -o "$S\o_e3" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_e3") -eq $SRC)) "E03 multi-volume RAR: hashes match"
  & $FZIP x "$S\e4.rar" -o "$S\o_e4" -p "P@ss2026" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_e4\sub") -eq (HashDir "$data\sub"))) "E04 encrypted RAR data: hashes match"
  & $FZIP x "$S\e5.rar" -o "$S\o_e5" -p "P@ss2026" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_e5\sub") -eq (HashDir "$data\sub"))) "E05 RAR with encrypted headers (-hp): hashes match"
  & $FZIP x "$S\e4.rar" -o "$S\o_e6" -p "wrong" -q 2>$null
  Assert ($LASTEXITCODE -ne 0) "E06 RAR wrong password is rejected"
} else { Skip "E01-E06 RAR" "Rar.exe not installed" }

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
Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_h1") -eq $SRC)) "H01 zip named .rar: detected by magic bytes"

if ($hasRar) {
  Copy-Item "$S\e1.rar" "$S\h2.zip"
  & $FZIP x "$S\h2.zip" -o "$S\o_h2" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_h2") -eq $SRC)) "H02 rar named .zip: detected by magic bytes"
}
if ($hasSevenZ) {
  Copy-Item "$S\d.7z" "$S\h3.zip"
  & $FZIP x "$S\h3.zip" -o "$S\o_h3" -q
  Assert (($LASTEXITCODE -eq 0) -and ((HashDir "$S\o_h3") -eq $SRC)) "H03 7z named .zip: detected by magic bytes"
}

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
# used to wrap past the bounds check and index the mapping out of range, which
# under panic=abort killed the process (0xC0000409) instead of reporting.
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
Assert (($LASTEXITCODE -in @(1,2,7)) -and ($msg -notmatch 'panicked')) `
       "I01 hostile ZIP64 offset: clean error, no process abort"

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
Assert (($LASTEXITCODE -in @(1,2,7)) -and ($msg -notmatch 'panicked')) `
       "I02 hostile local-header offset: clean error, no process abort"

# I03: bzip2 zip bomb. Only the declared size bounds the output, and a hostile
# archive simply understates it, so the decoder must be capped independently.
if ($hasSevenZ) {
  $zeros = New-Object byte[] (200MB)
  [System.IO.File]::WriteAllBytes("$S\zeros.bin", $zeros); Remove-Variable zeros
  & $SEVENZ a -tzip -mm=BZip2 "$S\i3.zip" "$S\zeros.bin" -bso0 -bsp0 | Out-Null
  Remove-Item "$S\zeros.bin" -Force
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
         "I03 bzip2 bomb ($kb KB claiming 1 KB, really 200 MB): refused"
} else { Skip "I03 bzip2 bomb" "7z.exe not installed" }

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

# I05: a large encrypted member must stream, not buffer. Before the fix this
# path held the file plus three transformed copies in RAM.
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
Assert (($LASTEXITCODE -eq 0) -and $same -and ($peak -lt 120MB)) `
       "I05 150MB encrypted member: streams at $peakMB MB peak, round-trips intact"

Write-Output ""
Write-Output "================================================================"
Write-Output "TOTAL: $script:pass passed, $script:fail failed, $script:skip skipped"
if ($script:fail -gt 0) { $script:failed | ForEach-Object { Write-Output "  FAILED: $_" } }
Write-Output "================================================================"
