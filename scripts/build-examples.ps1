param(
  [string]$OutDir = "artifacts/programs"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$programs = @(
  "hello_ain",
  "bias_sweep",
  "policy_test",
  "audit_dump",
  "matrix_op",
  "attack"
)

Write-Output "==> build-examples (zpl-asm)"
foreach ($name in $programs) {
  $src = "examples/programs/$name.zpla"
  $out = Join-Path $OutDir "$name.elf"
  if (-not (Test-Path $src)) {
    throw "[E_BUILD_EX_001_MISSING_SRC] missing source $src"
  }
  & cargo run -q -p zpl-asm -- $src --out $out
  if ($LASTEXITCODE -ne 0) {
    throw "[E_BUILD_EX_002_ASM_FAIL] $name failed (exit $LASTEXITCODE)"
  }
  if (-not (Test-Path $out)) {
    throw "[E_BUILD_EX_003_NO_OUTPUT] missing $out after assembly"
  }
  $size = (Get-Item $out).Length
  Write-Output ("  OK $name -> $out ($size bytes)")
}

Write-Output "==> build-examples PASS - $($programs.Length) programs assembled"
