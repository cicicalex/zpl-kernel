param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

Write-Output "==> replay-test step 1: generate fixtures"
& cargo run -q -p audit-replay -- gen-fixture --out artifacts/audit --records 5
if ($LASTEXITCODE -ne 0) { throw "[E_REPLAY_TEST_001] gen-fixture exit $LASTEXITCODE" }

Write-Output "==> replay-test step 2: verify clean chain (expect PASS)"
& cargo run -q -p audit-replay -- verify artifacts/audit/chain-good.json
if ($LASTEXITCODE -ne 0) {
  throw "[E_REPLAY_TEST_002] expected exit 0 on clean chain, got $LASTEXITCODE"
}

Write-Output "==> replay-test step 3: verify tampered chain (expect FAIL)"
$ErrorActionPreference = "Continue"
$null = & cargo run -q -p audit-replay -- verify artifacts/audit/chain-tampered.json 2>&1
$tamperExit = $LASTEXITCODE
$ErrorActionPreference = "Stop"
if ($tamperExit -eq 0) {
  throw "[E_REPLAY_TEST_003] expected non-zero exit on tampered chain, got 0"
}
Write-Output ("    tampered chain rejected with exit $tamperExit (expected non-zero)")

Write-Output "==> replay-test PASS - clean PASS, tampered FAIL detected"

# Step 3 deliberately runs a command that exits non-zero, and PowerShell keeps that
# value in $LASTEXITCODE. Without this line the script prints PASS and returns 1, so
# `full-verify.ps1`'s Run-Step reads a passing gate as a failure.
exit 0

