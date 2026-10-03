param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [string]$BenchJson = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/bench/bench_all.json"),
  [string]$Out = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "docs/benchmark-report.md")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $BenchJson)) {
  throw "[E_BENCH_REPORT_001_JSON_MISSING] Benchmark JSON not found: $BenchJson"
}

try {
  $rows = Get-Content -Path $BenchJson -Raw | ConvertFrom-Json
} catch {
  throw "[E_BENCH_REPORT_002_JSON_PARSE] Failed to parse benchmark JSON: $BenchJson"
}

if (-not $rows) {
  throw "[E_BENCH_REPORT_003_EMPTY_DATA] Benchmark JSON has no rows: $BenchJson"
}

$tableRows = @()
foreach ($row in $rows) {
  $tableRows += "| $($row.name) | $($row.repeats) | $([string]::Format('{0:N3}', $row.mean_ms)) | $([string]::Format('{0:N3}', $row.p95_ms)) |"
}

$content = @"
# Benchmark Report

## Harness

- Tool: `pb-bench`
- Cases: `small_3x3`, `mid_8x8`, `large_64x64`
- Metrics: mean latency, p95 latency

## Source

- JSON: `artifacts/bench/bench_all.json`

## Result table

| Case | Repeats | Mean (ms) | P95 (ms) |
|------|---------|-----------|----------|
$(($tableRows -join "`n"))
"@

[System.IO.File]::WriteAllText($Out, $content, [System.Text.UTF8Encoding]::new($false))
Write-Output "Benchmark report written: $Out"
