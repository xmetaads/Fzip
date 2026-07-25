# =====================================================================
#  fzip - benchmark against 7-Zip, WinRAR, tar and .NET
#  Usage:  .\run_bench.ps1
# =====================================================================
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$B = Join-Path $env:TEMP "fzip_bench"
if (Test-Path -LiteralPath $B) { Remove-Item -LiteralPath $B -Recurse -Force -Confirm:$false }
New-Item -ItemType Directory -Force $B | Out-Null

$FZIP = Join-Path $PSScriptRoot "fzip.exe"
$SEVENZ = "C:\Program Files\7-Zip\7z.exe"
$WINRAR = "C:\Program Files\WinRAR\WinRAR.exe"
$RAR    = "C:\Program Files\WinRAR\Rar.exe"
Add-Type -AssemblyName System.IO.Compression.FileSystem

Write-Output "CPU: $((Get-CimInstance Win32_Processor).Name.Trim()) / $env:NUMBER_OF_PROCESSORS threads"
Write-Output ""

# ---------- test data: ~190 MB, mixed text and binary ----------
$data = "$B\data"
New-Item -ItemType Directory -Force "$data\docs","$data\src" | Out-Null
$rnd = New-Object System.Random(42)
$base = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt. "
for ($i = 0; $i -lt 300; $i++) {
  $sb = New-Object System.Text.StringBuilder
  $reps = $rnd.Next(50, 5000)
  for ($j = 0; $j -lt $reps; $j++) { [void]$sb.Append($base).Append($i).Append(' ') }
  [System.IO.File]::WriteAllText("$data\$(@('docs','src')[$i % 2])\f$i.txt", $sb.ToString())
}
foreach ($k in 1..3) {
  $big = New-Object byte[] (40MB)
  $rnd.NextBytes($big)
  for ($m = 0; $m -lt $big.Length; $m += 2) { $big[$m] = 0 }
  [System.IO.File]::WriteAllBytes("$data\src\big$k.bin", $big)
}
$srcSize = (Get-ChildItem $data -Recurse -File | Measure-Object Length -Sum).Sum
Write-Output "Test data: $((Get-ChildItem $data -Recurse -File).Count) files, $([math]::Round($srcSize/1MB,1)) MB"

[System.IO.Compression.ZipFile]::CreateFromDirectory($data, "$B\test.zip", 'Optimal', $false, [System.Text.Encoding]::UTF8)
Write-Output "Test zip:  $([math]::Round((Get-Item "$B\test.zip").Length/1MB,1)) MB"
Write-Output ""

function Best($name, $runs, $block) {
  $times = @()
  for ($r = 1; $r -le $runs; $r++) {
    $dst = "$B\out_$($name -replace '[^a-zA-Z0-9]','')_$r"
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $block $dst
    $sw.Stop()
    $times += $sw.Elapsed.TotalSeconds
  }
  [PSCustomObject]@{ Tool = $name; Sec = [math]::Round(($times | Measure-Object -Minimum).Minimum, 3) }
}

function Report($rows, $bytes) {
  $fastest = ($rows | Measure-Object Sec -Minimum).Minimum
  $rows | Sort-Object Sec | ForEach-Object {
    $rel = if ($_.Sec -eq $fastest) { "fastest" } else { "{0}x slower" -f [math]::Round($_.Sec / $fastest, 2) }
    if ($bytes) {
      "{0,-14} {1,7:N3}s   {2,-14} ({3} MB/s)" -f $_.Tool, $_.Sec, $rel, [math]::Round($bytes/1MB/$_.Sec)
    } else {
      "{0,-14} {1,7:N3}s   {2}" -f $_.Tool, $_.Sec, $rel
    }
  }
}

Write-Output "===== 1. EXTRACT ZIP ($([math]::Round($srcSize/1MB,1)) MB) ====="
$res = @()
$res += Best "fzip" 3 { param($d) & $FZIP x "$B\test.zip" -o $d -q }
$res += Best "tar.exe" 3 { param($d) New-Item -ItemType Directory -Force $d | Out-Null; tar -xf "$B\test.zip" -C $d }
if (Test-Path $SEVENZ) { $res += Best "7-Zip" 3 { param($d) & $SEVENZ x "$B\test.zip" "-o$d" -y -bso0 -bsp0 | Out-Null } }
if (Test-Path $WINRAR) { $res += Best "WinRAR" 3 { param($d) New-Item -ItemType Directory -Force $d | Out-Null; & $WINRAR x -ibck -y "$B\test.zip" "$d\" | Out-Null } }
$res += Best ".NET ZipFile" 3 { param($d) [System.IO.Compression.ZipFile]::ExtractToDirectory("$B\test.zip", $d) }
Report $res $srcSize

Write-Output ""
Write-Output "===== 2. CREATE ZIP (default level) ====="
$res2 = @()
$res2 += Best "fzip -mx5" 3 { param($d) & $FZIP a "$d.zip" $data -y -q }
if (Test-Path $SEVENZ) { $res2 += Best "7-Zip -mx5" 3 { param($d) & $SEVENZ a -tzip -mx=5 "$d.zip" "$data\*" -bso0 -bsp0 | Out-Null } }
$res2 += Best ".NET ZipFile" 3 { param($d) [System.IO.Compression.ZipFile]::CreateFromDirectory($data, "$d.zip", 'Optimal', $false) }
if (Test-Path $RAR) { Write-Output "(WinRAR skipped: this Rar.exe build cannot create zip archives)" }
$fastest2 = ($res2 | Measure-Object Sec -Minimum).Minimum
$res2 | Sort-Object Sec | ForEach-Object {
  $out = Get-ChildItem "$B\out_$($_.Tool -replace '[^a-zA-Z0-9]','')_1.zip" -ErrorAction SilentlyContinue
  $sz = if ($out) { "$([math]::Round($out.Length/1MB,1)) MB" } else { "?" }
  $rel = if ($_.Sec -eq $fastest2) { "fastest" } else { "{0}x slower" -f [math]::Round($_.Sec / $fastest2, 2) }
  "{0,-14} {1,7:N3}s   {2,-14} output {3}" -f $_.Tool, $_.Sec, $rel, $sz
}

Write-Output ""
Write-Output "===== 3. VERIFY ONLY (no disk writes) ====="
# `fzip t` isolates decompression from storage, which is where the parallel
# advantage actually lives. 7-Zip's `t` is the same idea, so the pair compares.
$res3 = @()
$res3 += Best "fzip t" 3 { param($d) & $FZIP t "$B\test.zip" -q --no-pause | Out-Null }
if (Test-Path $SEVENZ) {
  $res3 += Best "7-Zip t" 3 { param($d) & $SEVENZ t "$B\test.zip" -bso0 -bsp0 | Out-Null }
}
Report $res3 $srcSize

Write-Output ""
Write-Output "===== 4. PEAK RAM extracting a single 200 MB member ====="
$big = New-Object byte[] (200MB)
for ($k = 0; $k -lt $big.Length; $k += 4096) { $big[$k] = [byte]($k % 251) }
[System.IO.File]::WriteAllBytes("$B\huge.bin", $big); Remove-Variable big
if (Test-Path $SEVENZ) {
  & $SEVENZ a -tzip -mx=1 "$B\huge.zip" "$B\huge.bin" -bso0 -bsp0 | Out-Null
  Remove-Item "$B\huge.bin" -Force
  foreach ($tool in @(@('fzip', $FZIP, @('x', "$B\huge.zip", '-o', "$B\hugeout_f", '-q')),
                      @('7-Zip', $SEVENZ, @('x', "$B\huge.zip", "-o$B\hugeout_7", '-y', '-bso0', '-bsp0')))) {
    $p = Start-Process -FilePath $tool[1] -ArgumentList $tool[2] -PassThru -NoNewWindow
    $peak = 0
    while (-not $p.HasExited) {
      Start-Sleep -Milliseconds 50
      try { $p.Refresh(); if ($p.WorkingSet64 -gt $peak) { $peak = $p.WorkingSet64 } } catch {}
    }
    "{0,-14} peak RAM {1,7:N1} MB" -f $tool[0], ($peak/1MB)
  }
}
Write-Output ""
Write-Output "Done."

