param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

Write-Output "==> fs-test step 1: cargo test -p zpl-fs"
& cargo test -p zpl-fs --quiet
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_001] cargo test exit $LASTEXITCODE" }

Write-Output "==> fs-test step 2: format empty image"
$img = "artifacts/fs/disk.img"
New-Item -ItemType Directory -Path "artifacts/fs" -Force | Out-Null
if (Test-Path $img) { Remove-Item $img -Force }
& cargo run -q -p zpl-mkfs -- format $img --blocks 32
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_002] mkfs format exit $LASTEXITCODE" }

Write-Output "==> fs-test step 3: pre-allocate audit chain inode"
& cargo run -q -p zpl-mkfs -- init-audit $img
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_003] mkfs init-audit exit $LASTEXITCODE" }

Write-Output "==> fs-test step 4: info"
$info = & cargo run -q -p zpl-mkfs -- info $img 2>&1
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_004] mkfs info exit $LASTEXITCODE" }
$infoText = $info -join "`n"
if (-not $infoText.Contains("blocks total:  32")) { throw "[E_FS_TEST_005] expected 32 blocks total in info" }
if (-not $infoText.Contains("files in use:  1")) { throw "[E_FS_TEST_006] expected 1 file in use after init-audit" }

Write-Output "==> fs-test step 5: end-to-end FS + audit chain"
& cargo run -q -p audit-replay -- gen-fixture --out artifacts/audit --records 5
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_007] gen-fixture exit $LASTEXITCODE" }
& cargo run -q -p audit-replay -- store-fs artifacts/audit/chain-good.json $img
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_008] store-fs exit $LASTEXITCODE" }
& cargo run -q -p audit-replay -- verify-fs $img
if ($LASTEXITCODE -ne 0) { throw "[E_FS_TEST_009] verify-fs exit $LASTEXITCODE" }

Write-Output "==> fs-test PASS - codec round-trip + mkfs + init-audit + info + audit-on-fs OK"
