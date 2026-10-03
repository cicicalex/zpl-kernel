param(
  [int]$Rounds = 2,
  [string]$ArtifactPath = "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

if ($Rounds -lt 2) {
  throw "[E_REPRO_001_ROUNDS] need at least 2 rounds (got $Rounds)"
}

function Sha256-File([string]$path) {
  if (-not (Test-Path $path)) {
    throw "[E_REPRO_002_MISSING_ARTIFACT] $path"
  }
  $bytes = [System.IO.File]::ReadAllBytes($path)
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $hash = $sha.ComputeHash($bytes)
  } finally {
    $sha.Dispose()
  }
  ($hash | ForEach-Object { '{0:x2}' -f $_ }) -join ''
}

$hashes = New-Object System.Collections.Generic.List[string]
for ($i = 1; $i -le $Rounds; $i++) {
  Write-Output "==> repro round $i / $Rounds : cargo clean -p zpl-kernel + zpl-kernel-bin"
  & cargo clean -p zpl-kernel
  if ($LASTEXITCODE -ne 0) { throw "[E_REPRO_010_CLEAN] cargo clean -p zpl-kernel exit $LASTEXITCODE" }
  & cargo clean -p zpl-kernel-bin
  if ($LASTEXITCODE -ne 0) { throw "[E_REPRO_011_CLEAN] cargo clean -p zpl-kernel-bin exit $LASTEXITCODE" }

  Write-Output "==> repro round $i / $Rounds : build kernel binary"
  & cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release
  if ($LASTEXITCODE -ne 0) {
    throw "[E_REPRO_020_BUILD] kernel build round $i exit $LASTEXITCODE"
  }

  $hash = Sha256-File $ArtifactPath
  $size = (Get-Item $ArtifactPath).Length
  $hashes.Add($hash)
  Write-Output ("    sha256 = " + $hash + " size=" + $size)
}

$first = $hashes[0]
$ok = $true
for ($j = 1; $j -lt $hashes.Count; $j++) {
  if ($hashes[$j] -ne $first) {
    $ok = $false
    Write-Output ("    MISMATCH round $($j + 1): " + $hashes[$j])
  }
}

$reportDir = Join-Path $root "artifacts/repro"
if (-not (Test-Path $reportDir)) {
  New-Item -ItemType Directory -Path $reportDir -Force | Out-Null
}
$reportPath = Join-Path $reportDir ("repro-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".log")
Set-Content -Path $reportPath -Value ($hashes -join "`n")

if ($ok) {
  Write-Output "==> repro-build PASS (same-machine deterministic): $first"
} else {
  Write-Error "[E_REPRO_030_DRIFT] kernel binary hashes differ across rounds. See $reportPath"
  exit 1
}
