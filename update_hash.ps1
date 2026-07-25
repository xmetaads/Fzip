# =====================================================================
#  Sync the published SHA-256 and byte size to the binary that will
#  actually be distributed.
#
#  Run this AFTER signing. Authenticode rewrites the file, so a hash taken
#  before signing does not match what anyone downloads - and the mismatch is
#  invisible to whoever published it, because they never re-check. That is the
#  single easiest way to make an honest release look tampered with.
#
#  Usage:  .\update_hash.ps1                 # after cargo build + signtool
#          .\update_hash.ps1 -WhatIf         # show what would change
# =====================================================================
[CmdletBinding(SupportsShouldProcess)]
param(
  [string]$Exe = (Join-Path $PSScriptRoot "target\release\fzip.exe")
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Exe)) {
  throw "not found: $Exe  - run: cargo build --release"
}

$hash    = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash
$size    = (Get-Item -LiteralPath $Exe).Length
$sizeCommas = '{0:N0}' -f $size
$version = (& $Exe -V | Select-Object -First 1) -replace '^\S+\s+',''

Write-Output "Binary : $Exe"
Write-Output "Version: $version"
Write-Output "Size   : $size bytes"
Write-Output "SHA-256: $hash"

# Report the signature, because publishing an unsigned build by accident is the
# other half of the same mistake.
$sig = Get-AuthenticodeSignature -LiteralPath $Exe
if ($sig.Status -eq 'Valid') {
  $subject = $sig.SignerCertificate.Subject
  Write-Output "Signed : $($sig.Status) - $subject"
} else {
  Write-Output "Signed : NO ($($sig.Status))"
  Write-Warning "This binary is not signed. If you intend to publish it signed, sign it FIRST, then re-run this script - signing changes the hash."
}
Write-Output ""

# Every place the CURRENT hash and size are published. Keeping the list here
# means one edit per release instead of five, and no file quietly left behind.
#
# MICROSOFT-REPLY.md is deliberately NOT in this list. It quotes the SHA-256 of
# the 1.0.1 binary as evidence in an open false-positive case, and a blanket
# replace overwrites that with the current hash - which was tried, and silently
# falsified the report. Files holding historical hashes get edited by hand.
$targets = @(
  'web\index.html'
  'web\usage.md'
  'web\llms.txt'
  'vercel.json'
  'README.md'
)

$utf8 = New-Object System.Text.UTF8Encoding($false)
$hashPattern = '\b[A-Fa-f0-9]{64}\b'
$changed = 0

foreach ($rel in $targets) {
  $path = Join-Path $PSScriptRoot $rel
  if (-not (Test-Path -LiteralPath $path)) { Write-Warning "missing: $rel"; continue }

  $text = [IO.File]::ReadAllText($path)
  $new  = [regex]::Replace($text, $hashPattern, $hash)
  # Size appears both bare (JSON, headers) and comma-grouped (prose).
  $new  = [regex]::Replace($new, '\b\d{1,3}(,\d{3})+ bytes\b', "$sizeCommas bytes")
  $new  = [regex]::Replace($new, '("(?:fileSize|X-Fzip-Size)"\s*[:,]\s*")\d+(")', "`${1}$size`${2}")

  if ($new -ne $text) {
    if ($PSCmdlet.ShouldProcess($rel, "update hash and size")) {
      [IO.File]::WriteAllText($path, $new, $utf8)
    }
    $n = ([regex]::Matches($text, $hashPattern)).Count
    Write-Output ("  updated {0,-22} ({1} hash occurrence{2})" -f $rel, $n, $(if ($n -eq 1) { '' } else { 's' }))
    $changed++
  } else {
    Write-Output ("  no change {0}" -f $rel)
  }
}

Write-Output ""
Write-Output "$changed file(s) updated."
Write-Output "Now verify nothing was missed:"
Write-Output "  Select-String -Path web\*,*.md,vercel.json -Pattern '[A-F0-9]{64}' | Group-Object { `$_.Matches[0].Value } | Select-Object Count, Name"
