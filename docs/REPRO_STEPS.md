# Reproduction Steps (Validation Day)

## Preconditions

- Rust toolchain installed
- A checkout of this repository

## Steps

1. `cargo check`

The lab validation steps use private tooling and are not part of the public export.

## Expected artifacts

- `artifacts/sim/normal_load.json`
- `artifacts/sim/fault_input.json`
- `artifacts/sim/integration.json`
- `artifacts/sim/integration_registry.json`
- `artifacts/sim/integration_registry_lifecycle.json`
- `artifacts/sim/integration_hsm_policy.json`
- `artifacts/sim/hsm_policy_summary.json`
- `artifacts/sim/signing_registry_snapshot.json`
- `artifacts/sim/integration_inputs.json`
- `artifacts/sim/policy_chain.json`
- `artifacts/sim/kernel_handoff_chain.json`
- `artifacts/sim/handoff_consistency.json`
- `artifacts/sim/replay_consistency.json`
- `artifacts/sim/replay_trend_summary.json`
- `artifacts/sim/safety.json`
- `artifacts/bench/bench_all.json`

## Expected quick checks

- `artifacts/sim/handoff_consistency.json` should have `mismatch_rate` near `0.0` for baseline run.
- `artifacts/sim/replay_consistency.json` should report both chain integrities as `true` and include `mode = deterministic_recompute` when replay inputs are provided.
- `artifacts/sim/integration.json` should report `signed_chain_integrity_ok = true`.
- `artifacts/sim/integration.json` should report `signed_chain_signatures_ok = true` when `--signer=dev` or `--signer=hsm-placeholder`.
- `artifacts/sim/integration_registry.json` should report `signed_chain_signatures_registry_ok = true` for active keys and `false` if signer key is listed in `revoked_key_ids`.
- `artifacts/sim/integration_hsm_policy.json` should report `hsm_trust_policy.ok = true` when policy and attestations match.
- `artifacts/sim/hsm_policy_summary.json` should summarize signer + trust policy gates and final `trustPolicyOk`.
- `artifacts/sim/signing_registry_snapshot.json` should include `rotation_log` and `attestation_log` entries when rotation/attestation flags are used.
- `artifacts/sim/replay_consistency.json` should fail run when mismatch rate exceeds `--replay-max-mismatch-rate`.
- `artifacts/sim/replay_consistency.json` should include `consistency_trends` with rolling mismatch windows.
- `artifacts/sim/replay_trend_summary.json` should summarize latest/peak mismatch rates from replay trend blocks.
- `artifacts/validation-evidence.json` should report `missingCount = 0` after full runbook.

## Lock contention troubleshooting (kernel serial path)

- If sink mirrors are enabled and you observe sparse console output, inspect boot-log health counters:
  - `LOG_BOOT_SINK_DROP_COUNT` indicates sink I/O lock contention (COM1/VGA mirror path).
  - `LOG_BOOT_RING_DROP_COUNT` indicates ring-buffer lock contention (entry not persisted in ring).
- Expected baseline for low-contention boot path:
  - both counters remain `0` or near `0` over short baseline runs.
- If `LOG_BOOT_RING_DROP_COUNT` grows:
  - increase ring lock spin budget via `set_ring_try_lock_spins(...)` in your test harness path before stress loops.
- If `LOG_BOOT_SINK_DROP_COUNT` grows:
  - reduce mirrored lanes (`apply_sink_lane_preset(...)` or lane mask controls) and/or increase sink spin budget via `set_sink_try_lock_spins(...)`.
