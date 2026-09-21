# SAPHOJUICE speed check, Windows.
#
#   irm https://saphojuice.com/check.ps1 | iex
#
# Downloads the probe for this CPU from the GitHub release, checks its SHA-256 against the
# SHA256SUMS published with that release, runs it, then deletes it. Nothing is installed, nothing
# is left behind, and no code signing is involved because the file does not persist.
#
# The binary is built in public CI from the source in this repository, so the hash you verify here
# is the hash of a build anyone can reproduce and inspect.
#
# Source of truth: https://github.com/saphojuice/speedcheck/blob/main/scripts/check.ps1

$ErrorActionPreference = 'Stop'

$Repo    = 'saphojuice/speedcheck'
$Version = if ($env:SJ_VERSION) { $env:SJ_VERSION } else { 'v0.1.0' }
$Base    = "https://github.com/$Repo/releases/download/$Version"

function Die($msg) {
  Write-Host ''
  Write-Host "  $msg" -ForegroundColor Red
  Write-Host ''
  exit 1
}

# ---- what are we on
$archRaw = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($archRaw) {
  'Arm64' { $arch = 'arm64' }
  'X64'   { $arch = 'x64' }
  default {
    Die @"
SAPHOJUICE: unsupported CPU architecture '$archRaw'.
  Builds exist for arm64 and x64. Please open an issue at
      https://github.com/$Repo/issues
  and say which machine you are on, and we will look at adding it.
"@
  }
}

$asset = "sj-probe-windows-$arch.exe"

# ---- scratch space that always goes away
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("sj-check-" + [guid]::NewGuid().ToString('N').Substring(0,12))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

$cleanup = {
  # deliberately unconditional: the binary must not survive this script, on any exit path
  if ($tmp -and (Test-Path $tmp)) { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
}

try {
  Write-Host ''
  Write-Host '  SAPHOJUICE speed check'
  Write-Host "  windows $arch, release $Version"
  Write-Host '  Downloading the probe (nothing is installed)...'

  $exe  = Join-Path $tmp $asset
  $sums = Join-Path $tmp 'SHA256SUMS'

  # ProgressPreference off: the built-in progress bar makes Invoke-WebRequest very slow
  $oldProgress = $ProgressPreference
  $ProgressPreference = 'SilentlyContinue'
  try {
    try { Invoke-WebRequest -Uri "$Base/$asset" -OutFile $exe -UseBasicParsing }
    catch { Die "SAPHOJUICE: could not download $Base/$asset`n  Check your connection, or that release $Version exists." }
    try { Invoke-WebRequest -Uri "$Base/SHA256SUMS" -OutFile $sums -UseBasicParsing }
    catch { Die "SAPHOJUICE: could not download SHA256SUMS for $Version.`n  Refusing to run an unverified binary." }
  } finally { $ProgressPreference = $oldProgress }

  $expected = $null
  foreach ($line in Get-Content $sums) {
    $parts = $line -split '\s+', 2
    if ($parts.Count -eq 2) {
      $name = $parts[1].Trim().TrimStart('*')
      if ($name -eq $asset) { $expected = $parts[0].Trim().ToLower(); break }
    }
  }
  if (-not $expected) {
    Die "SAPHOJUICE: $asset is not listed in SHA256SUMS for $Version.`n  Refusing to run an unverified binary."
  }

  $actual = (Get-FileHash -Path $exe -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $expected) {
    Die @"
SAPHOJUICE: SHA-256 MISMATCH. The download does not match the published release.

    expected  $expected
    actual    $actual

  The file has NOT been run and has been deleted. This could be a corrupted download, or
  something interfering with your connection. Try again; if it persists, please report it at
      https://github.com/$Repo/issues
"@
  }

  Write-Host "  SHA-256 verified: $expected"
  Write-Host '  Running. This takes about 30 seconds.'
  Write-Host ''

  # Call it directly so it inherits the console: the consent prompt needs a real keyboard, which
  # it would not get through a redirected pipe.
  & $exe @args
  $rc = $LASTEXITCODE

  Write-Host ''
  Write-Host '  Done. The probe has been deleted; nothing was installed.'
  Write-Host ''
  exit $rc
}
finally {
  & $cleanup
}
