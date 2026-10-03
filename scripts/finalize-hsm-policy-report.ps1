param(
  [string]$PolicyJson = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/sim/integration_hsm_policy.json"),
  [string]$OutJson = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "artifacts/sim/hsm_policy_summary.json"),
  [string]$OutMd = (Join-Path (Resolve-Path "$PSScriptRoot/..").Path "docs/hsm-policy-report.md")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path $PolicyJson)) {
  throw "[E_HSM_REPORT_001_POLICY_JSON_MISSING] HSM policy JSON not found: $PolicyJson"
}

try {
  $payload = Get-Content -Path $PolicyJson -Raw | ConvertFrom-Json
} catch {
  throw "[E_HSM_REPORT_002_POLICY_JSON_PARSE] Failed to parse HSM policy JSON: $PolicyJson"
}

if (-not $payload.hsm_trust_policy) {
  throw "[E_HSM_REPORT_003_POLICY_BLOCK_MISSING] Missing hsm_trust_policy block in JSON."
}

$hsm = $payload.hsm_trust_policy
$summary = [PSCustomObject]@{
  generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
  source = $PolicyJson
  signer = $payload.signer
  trustPolicyOk = [bool]$hsm.ok
  requireVerifiedAttestation = [bool]$hsm.require_verified_attestation
  allowedAttestors = @($hsm.allowed_attestors)
  requiredEvidenceUriPrefixes = @($hsm.required_evidence_uri_prefixes)
}

$outJsonDir = Split-Path -Parent $OutJson
if (-not (Test-Path $outJsonDir)) {
  New-Item -ItemType Directory -Force -Path $outJsonDir | Out-Null
}
[System.IO.File]::WriteAllText($OutJson, ($summary | ConvertTo-Json -Depth 8), [System.Text.UTF8Encoding]::new($false))

$md = @"
# HSM Trust Policy Report

## Source

- JSON: `$PolicyJson`

## Summary

- Signer: $($summary.signer)
- Trust policy OK: $($summary.trustPolicyOk)
- Require verified attestation: $($summary.requireVerifiedAttestation)

## Allowed attestors

$(if ($summary.allowedAttestors.Count -gt 0) { ($summary.allowedAttestors | ForEach-Object { "- $_" }) -join "`n" } else { "- (none)" })

## Required evidence URI prefixes

$(if ($summary.requiredEvidenceUriPrefixes.Count -gt 0) { ($summary.requiredEvidenceUriPrefixes | ForEach-Object { "- $_" }) -join "`n" } else { "- (none)" })
"@

$outMdDir = Split-Path -Parent $OutMd
if (-not (Test-Path $outMdDir)) {
  New-Item -ItemType Directory -Force -Path $outMdDir | Out-Null
}
[System.IO.File]::WriteAllText($OutMd, $md, [System.Text.UTF8Encoding]::new($false))

Write-Output "HSM policy summary written: $OutJson"
Write-Output "HSM policy markdown written: $OutMd"
