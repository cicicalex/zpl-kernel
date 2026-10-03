param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [ValidateSet("dev", "lab", "strict")]
  [string]$Profile = "lab"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Root)) {
  throw "[E_DAILY_001_ROOT_MISSING] Root path not found: $Root"
}

Set-Location $Root

$dateStamp = Get-Date -Format "yyyyMMdd"
$outDir = Join-Path "artifacts/daily" $dateStamp
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Write-Output "Running full non-interactive validation..."
powershell -ExecutionPolicy Bypass -File "scripts/tomorrow-validation.ps1" -Profile $Profile -SkipDashboard
if ($LASTEXITCODE -ne 0) {
  throw "[E_DAILY_002_VALIDATION_FAILED] tomorrow-validation failed with exit code ${LASTEXITCODE}"
}

$required = @(
  "artifacts/sim/replay_consistency.json",
  "artifacts/sim/handoff_consistency.json",
  "artifacts/validation-evidence.json"
)

foreach ($path in $required) {
  if (-not (Test-Path $path)) {
    throw "[E_DAILY_003_ARTIFACT_MISSING] Required artifact missing: $path"
  }
}

$replay = Get-Content "artifacts/sim/replay_consistency.json" -Raw | ConvertFrom-Json
$handoff = Get-Content "artifacts/sim/handoff_consistency.json" -Raw | ConvertFrom-Json
$evidence = Get-Content "artifacts/validation-evidence.json" -Raw | ConvertFrom-Json

$summary = [ordered]@{
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
  profile = $Profile
  replay = [ordered]@{
    policy_integrity_ok = $replay.policy_integrity_ok
    kernel_integrity_ok = $replay.kernel_integrity_ok
    chain_vs_chain_mismatch_rate = $replay.consistency.chain_vs_chain.mismatch_rate
    recompute_vs_policy_mismatch_rate = $replay.consistency.recompute_vs_policy_chain.mismatch_rate
    recompute_vs_kernel_mismatch_rate = $replay.consistency.recompute_vs_kernel_chain.mismatch_rate
  }
  handoff = [ordered]@{
    mismatch_rate = $handoff.mismatch_rate
    matches = $handoff.matches
    compared = $handoff.compared
  }
  evidence = [ordered]@{
    present_count = $evidence.presentCount
    missing_count = $evidence.missingCount
  }
}

$summaryJson = $summary | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText((Join-Path $outDir "daily-summary.json"), $summaryJson, [System.Text.Encoding]::UTF8)

Copy-Item "artifacts/sim/replay_consistency.json" (Join-Path $outDir "replay_consistency.json") -Force
Copy-Item "artifacts/sim/handoff_consistency.json" (Join-Path $outDir "handoff_consistency.json") -Force
Copy-Item "artifacts/validation-evidence.json" (Join-Path $outDir "validation-evidence.json") -Force

Write-Output "Daily stability check complete."
Write-Output "Output directory: $outDir"
