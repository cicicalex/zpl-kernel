param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [string]$Version = "0.1.0-prevalidation",
  [string]$OutDir = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/zenodo")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$required = @(
  "docs",
  "crates",
  "artifacts\bench",
  "artifacts\sim",
  "zenodo-metadata.json"
)

foreach ($rel in $required) {
  $abs = Join-Path $Root $rel
  if (-not (Test-Path $abs)) {
    throw "[E_ZENODO_001_REQUIRED_PATH_MISSING] Missing required path: $abs"
  }
}

if (-not (Test-Path (Join-Path $Root "artifacts\manifest.json"))) {
  throw "[E_ZENODO_002_MANIFEST_MISSING] Missing artifacts manifest. Run scripts/build-artifact-manifest.ps1 first."
}

if (-not (Test-Path $OutDir)) {
  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
}

$stageDir = Join-Path $OutDir "zpl-kernel-lab-$Version"
if (Test-Path $stageDir) {
  Remove-Item -Path $stageDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null

$copyList = @(
  "README.md",
  "LICENSE",
  "docs",
  "crates",
  "integration",
  "scripts",
  "artifacts\bench",
  "artifacts\sim",
  "artifacts\manifest.json",
  "zenodo-metadata.json"
)

foreach ($rel in $copyList) {
  $src = Join-Path $Root $rel
  if (-not (Test-Path $src)) {
    if ($rel -eq "LICENSE") {
      continue
    }
    throw "[E_ZENODO_003_COPY_SOURCE_MISSING] Missing source for package: $src"
  }
  $dst = Join-Path $stageDir $rel
  $dstParent = Split-Path -Parent $dst
  if (-not (Test-Path $dstParent)) {
    New-Item -ItemType Directory -Force -Path $dstParent | Out-Null
  }
  Copy-Item -Path $src -Destination $dst -Recurse -Force
}

$zipPath = Join-Path $OutDir "zpl-kernel-lab-$Version.zip"
if (Test-Path $zipPath) {
  Remove-Item -Path $zipPath -Force
}

try {
  Compress-Archive -Path (Join-Path $stageDir "*") -DestinationPath $zipPath -CompressionLevel Optimal
} catch {
  throw "[E_ZENODO_004_ARCHIVE_CREATE_FAILED] Failed to create archive: $zipPath"
}

$hash = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLower()
$summary = [PSCustomObject]@{
  version = $Version
  packageRoot = $stageDir
  archive = $zipPath
  archiveSha256 = $hash
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
}

$summaryPath = Join-Path $OutDir "package-summary.json"
$summary | ConvertTo-Json -Depth 6 | Set-Content -Path $summaryPath -Encoding UTF8

Write-Output "Zenodo package created:"
Write-Output "- Stage: $stageDir"
Write-Output "- Archive: $zipPath"
Write-Output "- SHA256: $hash"
Write-Output "- Summary: $summaryPath"
