param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [ValidateSet("dev", "lab", "strict")]
  [string]$Profile = "lab"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Root)) {
  throw "[E_PILOT_001_ROOT_MISSING] Root path not found: $Root"
}

Set-Location $Root

Write-Output "Running pilot preflight validation..."
powershell -ExecutionPolicy Bypass -File "scripts/daily-stability-check.ps1" -Profile $Profile
if ($LASTEXITCODE -ne 0) {
  throw "[E_PILOT_002_DAILY_CHECK_FAILED] daily stability check failed with exit code ${LASTEXITCODE}"
}

$summaryPath = Join-Path $Root ("artifacts/daily/" + (Get-Date -Format "yyyyMMdd") + "/daily-summary.json")
if (-not (Test-Path $summaryPath)) {
  throw "[E_PILOT_003_SUMMARY_MISSING] Missing daily summary: $summaryPath"
}

$summary = Get-Content $summaryPath -Raw | ConvertFrom-Json
$go = $true
$reasons = New-Object System.Collections.Generic.List[string]

if (-not $summary.replay.policy_integrity_ok) {
  $go = $false
  $reasons.Add("policy_integrity_ok is false")
}
if (-not $summary.replay.kernel_integrity_ok) {
  $go = $false
  $reasons.Add("kernel_integrity_ok is false")
}
if ($summary.replay.chain_vs_chain_mismatch_rate -ne 0.0) {
  $go = $false
  $reasons.Add("chain_vs_chain_mismatch_rate is not 0.0")
}
if ($summary.replay.recompute_vs_policy_mismatch_rate -ne 0.0) {
  $go = $false
  $reasons.Add("recompute_vs_policy_mismatch_rate is not 0.0")
}
if ($summary.replay.recompute_vs_kernel_mismatch_rate -ne 0.0) {
  $go = $false
  $reasons.Add("recompute_vs_kernel_mismatch_rate is not 0.0")
}
if ($summary.handoff.mismatch_rate -ne 0.0) {
  $go = $false
  $reasons.Add("handoff mismatch_rate is not 0.0")
}
if ($summary.evidence.missing_count -ne 0) {
  $go = $false
  $reasons.Add("evidence missing_count is not 0")
}

$result = [ordered]@{
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
  profile = $Profile
  summaryPath = $summaryPath
  pilotDay1Go = $go
  reasons = $reasons
}

$resultPath = Join-Path $Root ("artifacts/daily/" + (Get-Date -Format "yyyyMMdd") + "/pilot-day1-go-no-go.json")
$resultJson = $result | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText($resultPath, $resultJson, [System.Text.Encoding]::UTF8)

if ($go) {
  Write-Output "PILOT DAY-1 DECISION: GO"
} else {
  Write-Output "PILOT DAY-1 DECISION: NO-GO"
  foreach ($reason in $reasons) {
    Write-Output ("- " + $reason)
  }
}

Write-Output ("Decision file: " + $resultPath)
