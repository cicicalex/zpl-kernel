# SECURITY_MODEL_ZPL

## Security posture statement

ZPL Kernel Lab is designed to be **security-hardened** and **stability-aware**.
It does not claim unbreakable security.

## Claims we can make

- Attack resistance is increased by decision gating (`ALLOW/DEGRADE/BLOCK`).
- Critical actions are auditable and attributable.
- Faulty or high-bias inputs can be degraded or blocked by policy.

## Claims we must not make

- "Impossible to hack."
- "AGI-proof forever."
- "Cryptography replaced by ZPL."

## Security architecture layers

1. Cryptographic layer (standard algorithms only).
2. Identity and key management.
3. Policy enforcement layer (post-binary scheduler gating).
4. Audit trail and forensic layer.
5. Recovery and fail-safe layer.

## Cryptography baseline (target)

- Symmetric: AES-256-GCM or ChaCha20-Poly1305.
- Signing: Ed25519.
- Hashing: SHA-256/SHA-512.
- Password KDF: Argon2id.
- Randomness: OS CSPRNG only.

## ZPL-specific hardening hooks

- Decision gate before scheduling sensitive tasks.
- Bias anomaly guard for malformed workloads.
- Dynamic degrade mode for unstable states.
- Policy reason code for each block/degrade action.

## Key management principles

- No hardcoded keys in source.
- Key rotation and revocation by policy.
- Distinct keys per environment (dev/stage/prod).
- Secrets loaded at runtime through secure channels.
- HSM keys can be gated by trust policy (verified attestation, allowed attestors, evidence URI prefixes).

## HSM trust policy scaffold

`pb-integration` can enforce HSM trust policy checks before considering
`hsm-placeholder` signatures acceptable:

- require verified attestation for current key
- allow-list accepted attestors
- enforce evidence URI prefixes

This is a policy scaffold and does not replace production HSM custody controls.

## Threat categories

- Malicious input shaping (bias abuse).
- Scheduler starvation attack.
- Log tampering and forensic evasion.
- Key exfiltration attempts.
- Resource exhaustion (burst/fault storms).

## Control matrix (v0.1)

- Input validation + clamp: implemented.
- Deterministic scoring: implemented.
- Decision audit events: implemented.
- Fault campaign harness: scaffolded.
- Immutable signed logs: planned.
- Hardware-backed key storage: scaffold + trust policy enforcement hooks implemented, real HSM backend still pending.

## Incident response model

When a critical threshold breach is detected:
1. enter `degraded mode`;
2. block unsafe task classes;
3. preserve audit context;
4. recover using deterministic fallback path.

## Release gate (security)

Do not publish a release unless:
- thresholds are documented,
- safety report is attached,
- known limitations are explicit,
- no secrets are present in repository artifacts.
