param(
  [string]$Source = "examples/programs/hello_ain.zpla",
  [string]$OutPath = "artifacts/programs/hello_ain.elf"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

if (-not (Test-Path "artifacts/programs")) {
  New-Item -ItemType Directory -Path "artifacts/programs" -Force | Out-Null
}

Write-Output "==> zpl-asm build + assemble $Source"
& cargo run -q -p zpl-asm -- $Source --out $OutPath
if ($LASTEXITCODE -ne 0) {
  throw "[E_ASM_TEST_001_BUILD] zpl-asm exited with $LASTEXITCODE"
}
if (-not (Test-Path $OutPath)) {
  throw "[E_ASM_TEST_002_NO_OUTPUT] zpl-asm did not write $OutPath"
}

$bytes = [System.IO.File]::ReadAllBytes($OutPath)
Write-Output ("==> zpl-asm produced $($bytes.Length) bytes: $OutPath")

# Reference ELF: bit-identical match with `crate::elf_demo::HELLO_ELF`,
# which the kernel proved runnable via `scripts/elf-test.ps1`.
$expected = @(
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

if ($bytes.Length -ne $expected.Length) {
  throw "[E_ASM_TEST_003_LEN] expected $($expected.Length) bytes, got $($bytes.Length)"
}
for ($i = 0; $i -lt $bytes.Length; $i++) {
  if ($bytes[$i] -ne $expected[$i]) {
    $hexA = '{0:X2}' -f $bytes[$i]
    $hexB = '{0:X2}' -f $expected[$i]
    throw "[E_ASM_TEST_004_DIFF] byte $i differs: got 0x$hexA expected 0x$hexB"
  }
}

Write-Output "==> asm-test PASS - $($bytes.Length) bytes bit-identical with kernel-loadable HELLO_ELF"
