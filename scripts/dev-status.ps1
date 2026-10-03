param()

# One-shot workspace dashboard. Useful for daily check-in and the
# launch-day "is everything still green" question.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

function Section($title) {
  Write-Output ""
  Write-Output "============================================================"
  Write-Output "  $title"
  Write-Output "============================================================"
}

Section "1. git status"
$status = git status --porcelain 2>&1
if ([string]::IsNullOrWhiteSpace($status)) {
  Write-Output "  working tree clean"
}
else {
  Write-Output $status
}
$branch = (git rev-parse --abbrev-ref HEAD).Trim()
$head = (git log -1 --format='%h %s').Trim()
Write-Output "  branch: $branch"
Write-Output "  HEAD:   $head"

Section "2. workspace inventory"
$crates = Get-ChildItem -Path "crates" -Directory | Sort-Object Name
Write-Output "  $($crates.Count) crates:"
foreach ($c in $crates) {
  Write-Output "    - $($c.Name)"
}
$scripts = Get-ChildItem -Path "scripts" -Filter "*.ps1" | Sort-Object Name
Write-Output "  $($scripts.Count) PowerShell scripts:"
foreach ($s in $scripts) {
  Write-Output "    - $($s.Name)"
}

Section "3. recent kernel boot markers"
$lastLog = Get-ChildItem -Path "artifacts/kernel" -Filter "boot-run-*.log" -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime | Select-Object -Last 1
if ($null -ne $lastLog) {
  Write-Output "  log: $($lastLog.FullName) ($($lastLog.LastWriteTime))"
  $logText = Get-Content $lastLog.FullName | Where-Object { $_ -match "^\[ZPL-(BOOT|FRAME|HEAP|AUDIT|SMP|PERF)" }
  $bootMarkers = $logText | Select-Object -First 25
  foreach ($line in $bootMarkers) {
    Write-Output "    $line"
  }
}
else {
  Write-Output "  (no boot logs yet -- run scripts/qemu-run.ps1)"
}

Section "4. last firewall audit chain (if any)"
$lastChain = Get-ChildItem -Path "artifacts/audit" -Filter "firewall-*.json" -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime | Select-Object -Last 1
if ($null -ne $lastChain) {
  Write-Output "  chain: $($lastChain.FullName)"
  & cargo run -q -p audit-replay -- summary $lastChain.FullName 2>&1 | ForEach-Object { Write-Output "    $_" }
}
else {
  Write-Output "  (no firewall chains yet -- run scripts/firewall-test.ps1)"
}

Section "5. quick cargo check (workspace)"
$checkOut = & cargo check --workspace --quiet 2>&1
if ($LASTEXITCODE -eq 0) {
  Write-Output "  cargo check OK"
}
else {
  Write-Output "  cargo check FAIL ($LASTEXITCODE):"
  $checkOut | ForEach-Object { Write-Output "    $_" }
}

Section "6. disk free"
$free = [Math]::Round((Get-PSDrive C).Free / 1GB, 1)
Write-Output "  C: $free GB free (HARD STOP threshold: 2 GB)"

Write-Output ""
Write-Output "dev-status: done"
