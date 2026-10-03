param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [string]$ReplayJson = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/sim/replay_consistency.json"),
  [string]$IntegrationJson = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/sim/integration.json"),
  [string]$OutJson = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/sim/replay_trend_summary.json"),
  [string]$OutMd = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "docs/replay-trend-report.md")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $ReplayJson)) {
  throw "[E_REPLAY_REPORT_001_REPLAY_JSON_MISSING] Replay JSON not found: $ReplayJson"
}
if (-not (Test-Path $IntegrationJson)) {
  throw "[E_REPLAY_REPORT_002_INTEGRATION_JSON_MISSING] Integration JSON not found: $IntegrationJson"
}

try {
  $replay = Get-Content -Path $ReplayJson -Raw | ConvertFrom-Json
} catch {
  throw "[E_REPLAY_REPORT_003_REPLAY_JSON_PARSE] Failed to parse replay JSON: $ReplayJson"
}
try {
  $integration = Get-Content -Path $IntegrationJson -Raw | ConvertFrom-Json
} catch {
  throw "[E_REPLAY_REPORT_004_INTEGRATION_JSON_PARSE] Failed to parse integration JSON: $IntegrationJson"
}

$consistency = $replay.consistency
if (-not $consistency) {
  throw "[E_REPLAY_REPORT_005_REPLAY_CONSISTENCY_MISSING] Replay payload missing 'consistency' field."
}

$trendSource = $consistency.consistency_trends
if (-not $trendSource) {
  throw "[E_REPLAY_REPORT_006_REPLAY_TRENDS_MISSING] Replay payload missing 'consistency_trends'. Run deterministic replay mode with inputs."
}

$trendBlocks = @(
  [PSCustomObject]@{
    name = "chain_vs_chain_window_5"
    block = $trendSource.chain_vs_chain_window_5
  },
  [PSCustomObject]@{
    name = "chain_vs_chain_window_10"
    block = $trendSource.chain_vs_chain_window_10
  },
  [PSCustomObject]@{
    name = "recompute_vs_policy_window_5"
    block = $trendSource.recompute_vs_policy_window_5
  },
  [PSCustomObject]@{
    name = "recompute_vs_kernel_window_5"
    block = $trendSource.recompute_vs_kernel_window_5
  }
)

$normalized = @()
foreach ($entry in $trendBlocks) {
  $b = $entry.block
  if (-not $b) {
    continue
  }
  $pointsCount = 0
  if ($b.points) {
    $pointsCount = @($b.points).Count
  }
  $normalized += [PSCustomObject]@{
    name = $entry.name
    window = [int]$b.window
    pointsCount = $pointsCount
    latestMismatchRate = [double]$b.latest_mismatch_rate
    peakMismatchRate = [double]$b.peak_mismatch_rate
  }
}

if (@($normalized).Count -eq 0) {
  throw "[E_REPLAY_REPORT_007_TREND_BLOCKS_EMPTY] No trend blocks available in replay payload."
}

$handoffTrends = $integration.handoff_consistency_trends
$handoffWindow5Latest = $null
$handoffWindow10Latest = $null
if ($handoffTrends) {
  if ($handoffTrends.window_5) {
    $handoffWindow5Latest = [double]$handoffTrends.window_5.latest_mismatch_rate
  }
  if ($handoffTrends.window_10) {
    $handoffWindow10Latest = [double]$handoffTrends.window_10.latest_mismatch_rate
  }
}

$summary = [PSCustomObject]@{
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
  replayJson = $ReplayJson
  integrationJson = $IntegrationJson
  replayConsistencyMode = $consistency.mode
  replayTrendBlocks = $normalized
  handoffWindow5LatestMismatchRate = $handoffWindow5Latest
  handoffWindow10LatestMismatchRate = $handoffWindow10Latest
}

$outJsonDir = Split-Path -Parent $OutJson
if (-not (Test-Path $outJsonDir)) {
  New-Item -ItemType Directory -Force -Path $outJsonDir | Out-Null
}
[System.IO.File]::WriteAllText($OutJson, ($summary | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))

$rows = @()
foreach ($block in $normalized) {
  $rows += "| $($block.name) | $($block.window) | $($block.pointsCount) | $([string]::Format('{0:N6}', $block.latestMismatchRate)) | $([string]::Format('{0:N6}', $block.peakMismatchRate)) |"
}

$md = @"
# Replay Trend Report

## Source

- Replay JSON: `$ReplayJson`
- Integration JSON: `$IntegrationJson`

## Replay trend blocks

| Block | Window | Points | Latest mismatch rate | Peak mismatch rate |
|-------|--------|--------|----------------------|--------------------|
$(($rows -join "`n"))

## Handoff latest mismatch rates

- Window 5: $handoffWindow5Latest
- Window 10: $handoffWindow10Latest
"@

$outMdDir = Split-Path -Parent $OutMd
if (-not (Test-Path $outMdDir)) {
  New-Item -ItemType Directory -Force -Path $outMdDir | Out-Null
}
[System.IO.File]::WriteAllText($OutMd, $md, [System.Text.UTF8Encoding]::new($false))

Write-Output "Replay trend summary written: $OutJson"
Write-Output "Replay trend markdown written: $OutMd"
