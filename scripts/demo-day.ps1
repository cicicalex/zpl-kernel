param(
  [switch]$SkipRepro
)

# Aggregate demo runner (press-kit storyboard companion).
# Orchestrates the five user-facing scenarios so a launch capture can
# step through them in a single recording without juggling individual
# scripts.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

$results = @()

function Run-DemoStep {
  param(
    [string]$Title,
    [scriptblock]$Body
  )
  Write-Output "============================================================"
  Write-Output "DEMO STEP: $Title"
  Write-Output "============================================================"
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  try {
    & $Body
    $sw.Stop()
    $script:results += [PSCustomObject]@{
      Title  = $Title
      Status = "PASS"
      Sec    = "{0:N1}" -f ($sw.Elapsed.TotalSeconds)
    }
    Write-Output "    -> $Title PASS ($([int]$sw.Elapsed.TotalSeconds) s)"
  }
  catch {
    $sw.Stop()
    $script:results += [PSCustomObject]@{
      Title  = $Title
      Status = "FAIL"
      Sec    = "{0:N1}" -f ($sw.Elapsed.TotalSeconds)
    }
    Write-Output "    -> $Title FAIL: $($_.Exception.Message)"
    throw
  }
  Write-Output ""
}

Run-DemoStep "1. boot smoke (qemu-run + heap + audit selfcheck)" {
  $qemuProc = Start-Process -FilePath "powershell" `
    -ArgumentList @("-ExecutionPolicy","Bypass","-File","scripts/qemu-run.ps1") `
    -NoNewWindow -PassThru
  if (-not $qemuProc.WaitForExit(20000)) { Stop-Process -Id $qemuProc.Id -Force }
  Get-Process | Where-Object { $_.ProcessName -like "qemu-system-x86_64*" } |
    ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 800
  $latest = Get-ChildItem -Path "artifacts/kernel" -Filter "boot-run-*.log" |
    Sort-Object LastWriteTime | Select-Object -Last 1
  $log = (Get-Content $latest.FullName) -join "`n"
  if (-not $log.Contains("[ZPL-BOOT] kernel_entry reached"))     { throw "missing kernel_entry marker" }
  if (-not $log.Contains("[ZPL-FRAME] stress=1000000 leak=0"))  { throw "missing frame_alloc stress marker" }
  if (-not $log.Contains("[ZPL-HEAP] selfcheck box+vec100 OK")) { throw "missing heap selfcheck marker" }
  if (-not $log.Contains("[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK")) {
    throw "missing audit selfcheck marker"
  }
}

Run-DemoStep "2. demo killer (attack.zpla -> ZPL-SCHED BLOCK)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/attack-test.ps1"
  if ($LASTEXITCODE -ne 0) { throw "attack-test exit $LASTEXITCODE" }
}

Run-DemoStep "3. audit-replay (clean PASS, tampered FAIL)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/replay-test.ps1"
  if ($LASTEXITCODE -ne 0) { throw "replay-test exit $LASTEXITCODE" }
}

Run-DemoStep "4. zpl-fs persistence (mkfs + audit chain on disk)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/fs-test.ps1"
  if ($LASTEXITCODE -ne 0) { throw "fs-test exit $LASTEXITCODE" }
}

Run-DemoStep "5. zpl-firewall (clean ALLOW, attack DROP)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/firewall-test.ps1"
  if ($LASTEXITCODE -ne 0) { throw "firewall-test exit $LASTEXITCODE" }
}

if (-not $SkipRepro) {
  Run-DemoStep "6. reproducible build (3-round same-machine hash)" {
    & powershell -ExecutionPolicy Bypass -File "scripts/repro-build.ps1"
    if ($LASTEXITCODE -ne 0) { throw "repro-build exit $LASTEXITCODE" }
  }
}

Write-Output "============================================================"
Write-Output "DEMO DAY SUMMARY"
Write-Output "============================================================"
$results | Format-Table -AutoSize | Out-String | Write-Output

$failed = $results | Where-Object { $_.Status -eq "FAIL" }
if ($failed) {
  throw "[E_DEMO_DAY_FAIL] $($failed.Count) demo step(s) failed"
}
Write-Output "demo-day PASS - $($results.Count) scenarios green"
