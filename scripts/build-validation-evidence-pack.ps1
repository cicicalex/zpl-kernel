param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [string]$Out = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/validation-evidence.json")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Root)) {
  throw "[E_EVIDENCE_001_ROOT_MISSING] Root path not found: $Root"
}

$required = @(
  "artifacts/sim/normal_load.json",
  "artifacts/sim/fault_input.json",
  "artifacts/bench/bench_all.json",
  "artifacts/sim/integration.json",
  "artifacts/sim/integration_registry.json",
  "artifacts/sim/integration_registry_lifecycle.json",
  "artifacts/sim/integration_hsm_policy.json",
  "artifacts/sim/hsm_policy_summary.json",
  "artifacts/sim/signing_registry_snapshot.json",
  "artifacts/sim/integration_inputs.json",
  "artifacts/sim/policy_chain.json",
  "artifacts/sim/kernel_handoff_chain.json",
  "artifacts/sim/handoff_consistency.json",
  "artifacts/sim/replay_consistency.json",
  "artifacts/sim/replay_trend_summary.json",
  "artifacts/sim/safety.json"
)

$missing = @()
$present = @()

foreach ($rel in $required) {
  $abs = Join-Path $Root $rel
  if (Test-Path $abs) {
    $item = Get-Item $abs
    $present += [PSCustomObject]@{
      relativePath = $rel
      bytes = $item.Length
      lastWriteUtc = $item.LastWriteTimeUtc.ToString("o")
    }
  } else {
    $missing += $rel
  }
}

$manifestPath = Join-Path $Root "artifacts/manifest.json"
if (Test-Path $manifestPath) {
  $manifestHash = (Get-FileHash -Path $manifestPath -Algorithm SHA256).Hash.ToLower()
} else {
  $manifestHash = $null
}

$payload = [PSCustomObject]@{
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
  root = $Root
  requiredCount = $required.Count
  presentCount = $present.Count
  missingCount = $missing.Count
  missing = $missing
  present = $present
  artifactManifestPath = "artifacts/manifest.json"
  artifactManifestSha256 = $manifestHash
}

$json = $payload | ConvertTo-Json -Depth 8
$outDir = Split-Path -Parent $Out
if (-not (Test-Path $outDir)) {
  New-Item -ItemType Directory -Force -Path $outDir | Out-Null
}

[System.IO.File]::WriteAllText($Out, $json, [System.Text.UTF8Encoding]::new($false))
Write-Output "Validation evidence written: $Out"
