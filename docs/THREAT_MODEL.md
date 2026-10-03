# THREAT_MODEL

## Scope

Threat model for the research prototype (`zpl-kernel-lab`), not production.

## Assets

- Decision integrity (`ALLOW/DEGRADE/BLOCK` correctness)
- Audit integrity (events are preserved and attributable)
- Configuration integrity (thresholds and bounds)
- Availability (system does not fail catastrophically under stress)

## Adversary classes

1. Opportunistic attacker injecting malformed inputs.
2. Resource attacker attempting scheduler starvation.
3. Insider with configuration tampering intent.
4. Automation agent trying to force unstable decision loops.

## Entry points

- CLI inputs
- Scenario configuration payloads
- Future IPC interfaces
- Build/release artifact handling

## Abuse cases

- Bias forcing via repeated extreme values.
- Dimension/sample abuse to trigger pathological compute paths.
- Threshold downgrade to bypass block policies.
- Audit suppression attempts.

## Controls (current and planned)

- Input clamps and validation (current).
- Policy threshold floor checks (current).
- Decision reason codes (current).
- Signed audit chain (planned).
- Config signature verification (planned).

## Residual risks

- No cryptographic attestation in v0.1.
- No secure boot guarantees in lab stage.
- No production hard real-time guarantees.
