param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [string]$Out = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/manifest.json")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$artifactsPath = Join-Path $Root "artifacts"
if (-not (Test-Path $artifactsPath)) {
  throw "[E_MANIFEST_001_ARTIFACTS_PATH_MISSING] Artifacts path not found: $artifactsPath"
}

$files = Get-ChildItem -Path $artifactsPath -Recurse -File | ForEach-Object {
  $hash = (Get-FileHash -Path $_.FullName -Algorithm SHA256).Hash.ToLower()
  [PSCustomObject]@{
    relativePath = $_.FullName.Replace($Root + "\", "")
    bytes = $_.Length
    lastWriteUtc = $_.LastWriteTimeUtc.ToString("o")
    sha256 = $hash
  }
}

$files = $files | Sort-Object relativePath

$manifest = [PSCustomObject]@{
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
  root = $Root
  count = $files.Count
  formatVersion = "1.1"
  files = $files
}

$json = $manifest | ConvertTo-Json -Depth 8
$outDir = Split-Path -Parent $Out
if (-not (Test-Path $outDir)) {
  New-Item -ItemType Directory -Force -Path $outDir | Out-Null
}
[System.IO.File]::WriteAllText($Out, $json, [System.Text.UTF8Encoding]::new($false))
Write-Output "Manifest written: $Out"
