# SIGNED_AUDIT_CHAIN_FORMAT

## Purpose

Define a tamper-evident audit format for post-binary decisions.

## Entry schema

```json
{
  "index": 12,
  "prev_hash": "a1b2...ff",
  "event": {
    "ts": "2026-04-17T22:31:00Z",
    "task_id": "task-012",
    "decision": "Degrade",
    "score": 0.63,
    "reason": "anti_oscillation_triggered"
  },
  "hash": "f0e1...aa",
  "signature_provider": "deterministic-dev",
  "signature_algorithm": "sha256-dev-detached",
  "signature_key_id": "local-dev-key",
  "signature": null
}
```

## Hash composition (v0 scaffold)

`SHA256(index || prev_hash || ts || task_id || decision_tag || score || reason)`

Where:
- `decision_tag`: `0=Allow`, `1=Degrade`, `2=Block`
- `prev_hash` is `"GENESIS"` for the first entry

## Verification rules

1. `index` increases by 1 each entry.
2. `entry.prev_hash == previous.entry.hash`
3. Recomputed hash equals stored `hash`
4. If signature metadata exists, `provider` + `algorithm` + `key_id` must be coherent and verifier-compatible.
5. Signature validation supports:
   - `deterministic-dev` (test only),
   - `hsm-placeholder` (integration scaffold),
   - `ed25519` detached software signatures.
   Production HSM key management and secure key storage remain pending.

## Upgrade path

- v0.2: HSM-backed ed25519 provider + key rotation metadata (scaffold now available in registry snapshot/export path)
- v0.3: trust anchor block and remote attestation integration

## Registry lifecycle scaffold (v0.1-prevalidation)

`pb-integration` supports registry lifecycle orchestration via:

- `--signing-registry-in`
- `--signing-registry-out`
- `--signing-revoke-key`
- `--signing-rotate-from` / `--signing-rotate-to`
- `--signing-attest-key-id`
- `--hsm-require-verified-attestation`
- `--hsm-allowed-attestor`
- `--hsm-required-evidence-uri-prefix`
- `--hsm-trust-policy-in`
- `--hsm-enforce-trust-policy`

The registry snapshot format includes:

- `keys`
- `revoked_key_ids`
- `rotation_log`
- `attestation_log`

This is still a software scaffold and not a replacement for production HSM custody.

## HSM trust policy scaffold

For `--signer=hsm-placeholder`, registry verification can be strengthened with trust policy gates:

- require latest key attestation status `Verified`
- restrict accepted attestors by allow-list
- require evidence URI prefixes

The resulting integration payload exposes `hsm_trust_policy.ok` to make policy decisions auditable.

Policy gates include:

- `require_verified_attestation`
- `allowed_attestors[]`
- `required_evidence_uri_prefixes[]`

If `--hsm-enforce-trust-policy` is set and policy validation fails, run exits with:
- `[E_INT_030_HSM_POLICY_ENFORCEMENT_FAILED]`
