param(
  [int]$TimeoutSeconds = 25
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

Write-Output "==> e2e step 1: assemble hello_ain.zpla via zpl-asm"
& powershell -ExecutionPolicy Bypass -File "scripts/asm-test.ps1"
if ($LASTEXITCODE -ne 0) {
  throw "[E_E2E_001_ASM] asm-test failed (exit $LASTEXITCODE)"
}

Write-Output "==> e2e step 2: load + run via kernel ELF demo"
& powershell -ExecutionPolicy Bypass -File "scripts/elf-test.ps1"
if ($LASTEXITCODE -ne 0) {
  throw "[E_E2E_002_ELF] elf-test failed (exit $LASTEXITCODE)"
}

Write-Output "==> e2e step 3: confirm hello_ain.elf is bit-identical to embedded HELLO_ELF"
$asmOut = "artifacts/programs/hello_ain.elf"
if (-not (Test-Path $asmOut)) {
  throw "[E_E2E_003_NO_ASM_ELF] $asmOut missing after asm-test"
}
$asmBytes = [System.IO.File]::ReadAllBytes($asmOut)
$elfBytes = @(
  0x7F, 0x45, 0x4C, 0x46, 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
  2, 0, 0x3E, 0, 1, 0, 0, 0,
  0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
  64, 0, 0, 0, 0, 0, 0, 0,
  0, 0, 0, 0, 0, 0, 0, 0,
  0, 0, 0, 0,
  64, 0, 56, 0, 1, 0, 0, 0, 0, 0, 0, 0,
  1, 0, 0, 0, 7, 0, 0, 0,
  120, 0, 0, 0, 0, 0, 0, 0,
  0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
  0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0x00,
  31, 0, 0, 0, 0, 0, 0, 0,
  31, 0, 0, 0, 0, 0, 0, 0,
  1, 0, 0, 0, 0, 0, 0, 0,
  0xB8, 0x00, 0x00, 0x00, 0x00,
  0xBF, 0x1D, 0x00, 0x01, 0x40,
  0xBE, 0x02, 0x00, 0x00, 0x00,
  0xCD, 0x80,
  0xB8, 0x04, 0x00, 0x00, 0x00,
  0xBF, 0x00, 0x00, 0x00, 0x00,
  0xCD, 0x80,
  0x68, 0x69
)
if ($asmBytes.Length -ne $elfBytes.Length) {
  throw "[E_E2E_004_DIFF_LEN] zpl-asm produced $($asmBytes.Length) bytes, kernel HELLO_ELF is $($elfBytes.Length)"
}
for ($i = 0; $i -lt $asmBytes.Length; $i++) {
  if ($asmBytes[$i] -ne $elfBytes[$i]) {
    throw "[E_E2E_005_DIFF_BYTE] byte $i differs"
  }
}

Write-Output "==> e2e step 4: scan latest elf-run log for syscall + exit markers"
$latest = Get-ChildItem -Path "artifacts/kernel" -Filter "elf-run-*.log" |
  Sort-Object LastWriteTime |
  Select-Object -Last 1
if (-not $latest) { throw "[E_E2E_006_NO_LOG] missing elf-run log" }
$log = Get-Content -Path $latest.FullName -Raw
foreach ($marker in @(
  "[ZPL-ELF] load ok",
  "[ZPL-ELF] entry=0x0000000040010000",
  "[ZPL-SYSCALL] log: hi",
  "[ZPL-SYSCALL] exit code=0",
  "[ZPL-RING] qemu_exit=success"
)) {
  if (-not $log.Contains($marker)) {
    throw "[E_E2E_007_MISSING] log missing marker: $marker"
  }
}

Write-Output "==> e2e-test PASS - source -> zpl-asm -> ELF (bit-identical) -> kernel ELF loader -> ring 3 -> SYS_LOG -> SYS_EXIT -> qemu_exit"
