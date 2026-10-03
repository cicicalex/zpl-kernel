param(
  [int]$RunSeconds = 3,
  [int]$MemoryMb = 256,
  [switch]$KeepLog
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

$artifactDir = Join-Path $root "artifacts/kernel"
if (-not (Test-Path $artifactDir)) {
  New-Item -ItemType Directory -Path $artifactDir | Out-Null
}

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$serialLog = Join-Path $artifactDir "m5-demo-$timestamp.log"
$summaryLog = Join-Path $artifactDir "m5-demo-$timestamp.summary.txt"

Write-Output "==> Build kernel-bin (release)"
$env:RUSTUP_OFFLINE = "1"
& cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release | Out-Null
if ($LASTEXITCODE -ne 0) {
  throw "[E_M5_BUILD] cargo build failed (exit $LASTEXITCODE)"
}

$artifact = Join-Path $root "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
if (-not (Test-Path $artifact)) {
  throw "[E_M5_ARTIFACT] kernel-bin not found at $artifact"
}

Write-Output "==> ELF gate"
$llvmDir = Join-Path $env:USERPROFILE ".rustup/toolchains/nightly-x86_64-pc-windows-msvc/lib/rustlib/x86_64-pc-windows-msvc/bin"
$objdump = Join-Path $llvmDir "llvm-objdump.exe"
$readobj = Join-Path $llvmDir "llvm-readobj.exe"
& $objdump -f $artifact
& $readobj --file-headers $artifact | Select-String "Type:|Machine:|Entry:"

Write-Output "==> Kill stale qemu"
Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

Write-Output "==> Run QEMU for $RunSeconds s, capturing serial to:"
Write-Output "    $serialLog"
$qemu = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
if (-not $qemu) {
  throw "[E_M5_QEMU] qemu-system-x86_64 not on PATH"
}
$qemuArgs = @(
  "-kernel", $artifact,
  "-serial", "file:$serialLog",
  "-display", "none",
  "-no-reboot",
  "-no-shutdown",
  "-m", "$MemoryMb"
)
$proc = Start-Process -FilePath $qemu.Source -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
Start-Sleep -Seconds $RunSeconds
if (-not $proc.HasExited) {
  Stop-Process -Id $proc.Id -Force
}
Start-Sleep -Milliseconds 500

if (-not (Test-Path $serialLog)) {
  throw "[E_M5_SERIAL] no serial log produced"
}

$content = @(Get-Content $serialLog)
$lineCount = $content.Count
function Count-Match([object[]]$lines, [string]$pattern) {
  return @($lines | Select-String $pattern).Count
}
$tickCount = Count-Match $content "TICK"
$schedCount = Count-Match $content "ZPL-SCHED"
$tidA = Count-Match $content "tid=A"
$tidB = Count-Match $content "tid=B"
$tidC = Count-Match $content "tid=C"
$allow = Count-Match $content "action=ALLOW"
$degrade = Count-Match $content "action=DEGRADE"
$block = Count-Match $content "action=BLOCK"
$bootMarkers = Count-Match $content "ZPL-BOOT"

$report = @(
  "ZPL Kernel Lab - M5 Demo Summary",
  "Timestamp: $timestamp",
  "Run window seconds: $RunSeconds",
  "Serial log: $serialLog",
  "",
  "Total serial lines: $lineCount",
  "Boot markers: $bootMarkers (expected 3)",
  "Scheduler decisions: $schedCount",
  "  tid=A: $tidA",
  "  tid=B: $tidB",
  "  tid=C: $tidC",
  "Decisions:",
  "  ALLOW:   $allow",
  "  DEGRADE: $degrade",
  "  BLOCK:   $block",
  "",
  "First 12 lines:"
)
$report += ($content | Select-Object -First 12 | ForEach-Object { "  $_" })
$report = $report -join [Environment]::NewLine

[System.IO.File]::WriteAllText($summaryLog, $report)
Write-Output ""
Write-Output $report
Write-Output ""
Write-Output "Summary saved to: $summaryLog"

if (-not $KeepLog) {
  Write-Output "(Pass -KeepLog to retain raw serial log; current behaviour: kept by default)"
}

if ($bootMarkers -lt 3) {
  Write-Warning "[W_M5_BOOT] expected 3 boot markers, observed $bootMarkers"
}
if ($schedCount -lt 10) {
  Write-Warning "[W_M5_SCHED] expected at least 10 scheduler decisions, observed $schedCount"
}
