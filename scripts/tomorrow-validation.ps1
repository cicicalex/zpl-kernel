param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path,
  [ValidateSet("dev", "lab", "strict")]
  [string]$Profile = "lab",
  [switch]$SkipDashboard,
  [switch]$SkipIntegration
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Root)) {
  throw "[E_VALIDATE_001_ROOT_MISSING] Root path not found: $Root"
}

Set-Location $Root

New-Item -ItemType Directory -Force -Path "artifacts/sim" | Out-Null
New-Item -ItemType Directory -Force -Path "artifacts/bench" | Out-Null
New-Item -ItemType Directory -Force -Path "artifacts/kernel" | Out-Null

function Invoke-NativeCommand {
  param(
    [string]$ErrorCode,
    [string]$Executable,
    [string[]]$Arguments
  )

  & $Executable @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "[$ErrorCode] Command failed with exit code ${LASTEXITCODE}: $Executable $($Arguments -join ' ')"
  }
}

function Assert-RequiredPath {
  param(
    [string]$ErrorCode,
    [string]$Path
  )

  if (-not (Test-Path $Path)) {
    throw "[$ErrorCode] Required path missing: $Path"
  }
}

Assert-RequiredPath -ErrorCode "E_VALIDATE_019_SIGNING_REGISTRY_MISSING" -Path "config/signing-registry.example.json"
Assert-RequiredPath -ErrorCode "E_VALIDATE_020_HSM_POLICY_MISSING" -Path "config/hsm-trust-policy.example.json"
Assert-RequiredPath -ErrorCode "E_VALIDATE_021_BENCH_REPORT_SCRIPT_MISSING" -Path "scripts/finalize-benchmark-report.ps1"
Assert-RequiredPath -ErrorCode "E_VALIDATE_022_HSM_REPORT_SCRIPT_MISSING" -Path "scripts/finalize-hsm-policy-report.ps1"
Assert-RequiredPath -ErrorCode "E_VALIDATE_023_REPLAY_REPORT_SCRIPT_MISSING" -Path "scripts/finalize-replay-trend-report.ps1"
Assert-RequiredPath -ErrorCode "E_VALIDATE_024_MANIFEST_SCRIPT_MISSING" -Path "scripts/build-artifact-manifest.ps1"
Assert-RequiredPath -ErrorCode "E_VALIDATE_025_EVIDENCE_SCRIPT_MISSING" -Path "scripts/build-validation-evidence-pack.ps1"

$cargoCmd = Get-Command "cargo" -ErrorAction SilentlyContinue
if (-not $cargoCmd) {
  throw "[E_VALIDATE_026_CARGO_MISSING] cargo command not found on PATH."
}

$powershellCmd = Get-Command "powershell" -ErrorAction SilentlyContinue
if (-not $powershellCmd) {
  throw "[E_VALIDATE_027_POWERSHELL_MISSING] powershell command not found on PATH."
}

Write-Output "Validation day runbook (live commands)"
Write-Output "Profile: $Profile"
Write-Output "1) cargo check"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_002_CARGO_CHECK_FAILED" -Executable "cargo" -Arguments @("check")

Write-Output "2) pb-cli compute sample"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_003_PB_CLI_FAILED" -Executable "cargo" -Arguments @(
  "run", "-p", "pb-cli", "--",
  "compute", "--profile", $Profile, "--bias", "0.5", "--dimension", "9", "--samples", "1000", "--json"
)

Write-Output "3) pb-sim normal_load -> artifacts/sim/normal_load.json"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_004_PB_SIM_NORMAL_FAILED" -Executable "cargo" -Arguments @(
  "run", "-p", "pb-sim", "--",
  "--profile", $Profile, "--scenario", "normal_load", "--out", "artifacts/sim/normal_load.json"
)

Write-Output "4) pb-sim fault_input -> artifacts/sim/fault_input.json"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_005_PB_SIM_FAULT_FAILED" -Executable "cargo" -Arguments @(
  "run", "-p", "pb-sim", "--",
  "--profile", $Profile, "--scenario", "fault_input", "--out", "artifacts/sim/fault_input.json"
)

Write-Output "5) pb-bench all -> artifacts/bench/bench_all.json"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_006_PB_BENCH_FAILED" -Executable "cargo" -Arguments @(
  "run", "-p", "pb-bench", "--",
  "--profile", $Profile, "--scenario", "all", "--repeats", "5", "--out", "artifacts/bench/bench_all.json"
)

Write-Output "5.1) finalize benchmark report"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_007_BENCH_REPORT_FAILED" -Executable "powershell" -Arguments @(
  "-ExecutionPolicy", "Bypass", "-File", "scripts/finalize-benchmark-report.ps1"
)

if (-not $SkipIntegration) {
  Write-Output "6) pb-integration with handoff exports"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_008_PB_INT_BASE_FAILED" -Executable "cargo" -Arguments @(
    "run", "-p", "pb-integration", "--",
    "--profile", $Profile,
    "--tasks", "20",
    "--signer", "dev",
    "--signer-key-id", "validation-dev-key",
    "--out", "artifacts/sim/integration.json",
    "--inputs-out", "artifacts/sim/integration_inputs.json",
    "--audit-chain-out", "artifacts/sim/policy_chain.json",
    "--kernel-handoff-chain-out", "artifacts/sim/kernel_handoff_chain.json",
    "--consistency-out", "artifacts/sim/handoff_consistency.json"
  )

  Write-Output "6.1) pb-integration registry import"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_009_PB_INT_REGISTRY_FAILED" -Executable "cargo" -Arguments @(
    "run", "-p", "pb-integration", "--",
    "--profile", $Profile,
    "--tasks", "20",
    "--signer", "dev",
    "--signer-key-id", "validation-dev-key",
    "--signing-registry-in", "config/signing-registry.example.json",
    "--out", "artifacts/sim/integration_registry.json"
  )

  Write-Output "6.2) pb-integration lifecycle rotation + attestation + registry export"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_010_PB_INT_LIFECYCLE_FAILED" -Executable "cargo" -Arguments @(
    "run", "-p", "pb-integration", "--",
    "--profile", $Profile,
    "--tasks", "20",
    "--signer", "dev",
    "--signer-key-id", "validation-dev-key",
    "--signing-registry-in", "config/signing-registry.example.json",
    "--signing-rotate-from", "validation-dev-key",
    "--signing-rotate-to", "validation-dev-key-v2",
    "--signing-rotate-provider", "deterministic-dev",
    "--signing-rotate-algorithm", "sha256-dev-detached",
    "--signing-attest-key-id", "validation-dev-key-v2",
    "--signing-attestor", "lab-attestor",
    "--signing-attestation-status", "verified",
    "--signing-attestation-custody-tier", "hsm",
    "--signing-registry-out", "artifacts/sim/signing_registry_snapshot.json",
    "--out", "artifacts/sim/integration_registry_lifecycle.json"
  )

  Write-Output "6.3) pb-integration HSM trust policy validation"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_011_PB_INT_HSM_FAILED" -Executable "cargo" -Arguments @(
    "run", "-p", "pb-integration", "--",
    "--profile", $Profile,
    "--tasks", "20",
    "--signer", "dev",
    "--signer-key-id", "validation-dev-key-v2",
    "--signing-registry-in", "artifacts/sim/signing_registry_snapshot.json",
    "--hsm-trust-policy-in", "config/hsm-trust-policy.example.json",
    "--hsm-enforce-trust-policy",
    "--out", "artifacts/sim/integration_hsm_policy.json"
  )

  Write-Output "6.4) finalize HSM policy report"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_012_HSM_REPORT_FAILED" -Executable "powershell" -Arguments @(
    "-ExecutionPolicy", "Bypass", "-File", "scripts/finalize-hsm-policy-report.ps1"
  )

  Write-Output "7) pb-integration replay consistency from persisted chains"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_013_PB_INT_REPLAY_FAILED" -Executable "cargo" -Arguments @(
    "run", "-p", "pb-integration", "--",
    "--profile", $Profile,
    "--replay-max-mismatch-rate", "0.0",
    "--replay-policy-chain-in", "artifacts/sim/policy_chain.json",
    "--replay-kernel-chain-in", "artifacts/sim/kernel_handoff_chain.json",
    "--replay-inputs-in", "artifacts/sim/integration_inputs.json",
    "--replay-consistency-out", "artifacts/sim/replay_consistency.json"
  )

  Write-Output "7.1) finalize replay trend report"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_014_REPLAY_REPORT_FAILED" -Executable "powershell" -Arguments @(
    "-ExecutionPolicy", "Bypass", "-File", "scripts/finalize-replay-trend-report.ps1"
  )
} else {
  Write-Output "6) pb-integration and replay steps skipped (--SkipIntegration)"
}

Write-Output "8) pb-safety -> artifacts/sim/safety.json"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_015_PB_SAFETY_FAILED" -Executable "cargo" -Arguments @(
  "run", "-p", "pb-safety", "--",
  "--profile", $Profile, "--steps", "30", "--out", "artifacts/sim/safety.json"
)

if (-not $SkipDashboard) {
  Write-Output "9) pb-dashboard manual review"
  Invoke-NativeCommand -ErrorCode "E_VALIDATE_016_PB_DASHBOARD_FAILED" -Executable "cargo" -Arguments @(
    "run", "-p", "pb-dashboard", "--",
    "--profile", $Profile, "--scenario", "burst_load"
  )
} else {
  Write-Output "9) pb-dashboard manual review skipped (--SkipDashboard)"
}

Write-Output "10) build artifact manifest"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_017_MANIFEST_FAILED" -Executable "powershell" -Arguments @(
  "-ExecutionPolicy", "Bypass", "-File", "scripts/build-artifact-manifest.ps1"
)

Write-Output "11) build validation evidence pack"
Invoke-NativeCommand -ErrorCode "E_VALIDATE_018_EVIDENCE_PACK_FAILED" -Executable "powershell" -Arguments @(
  "-ExecutionPolicy", "Bypass", "-File", "scripts/build-validation-evidence-pack.ps1"
)
