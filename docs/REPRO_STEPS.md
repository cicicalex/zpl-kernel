# Reproduction Steps (Validation Day)

## Preconditions

- Rust toolchain installed
- Workspace at `c:\Dev\zpl-kernel-lab`

## Steps

1. `cargo check`
2. `cargo run -p pb-cli -- compute --profile lab --bias 0.5 --dimension 9 --samples 1000 --json`
3. `cargo run -p pb-sim -- --profile lab --scenario normal_load --out artifacts/sim/normal_load.json`
4. `cargo run -p pb-sim -- --profile lab --scenario fault_input --anti-oscillation --guard-flip-threshold 3 --guard-window-size 6 --guard-cooldown-steps 2 --guard-cooldown-escalation-step 1 --guard-max-cooldown-steps 8 --guard-flip-decay 1 --out artifacts/sim/fault_input.json`
5. `cargo run -p pb-bench -- --profile lab --scenario all --repeats 5 --out artifacts/bench/bench_all.json`
6. `cargo run -p pb-integration -- --profile lab --tasks 20 --signer dev --signer-key-id validation-dev-key --out artifacts/sim/integration.json --inputs-out artifacts/sim/integration_inputs.json --audit-chain-out artifacts/sim/policy_chain.json --kernel-handoff-chain-out artifacts/sim/kernel_handoff_chain.json --consistency-out artifacts/sim/handoff_consistency.json`
7. `cargo run -p pb-integration -- --profile lab --tasks 20 --signer dev --signer-key-id validation-dev-key --signing-registry-in config/signing-registry.example.json --out artifacts/sim/integration_registry.json`
8. `cargo run -p pb-integration -- --profile lab --tasks 20 --signer dev --signer-key-id validation-dev-key --signing-registry-in config/signing-registry.example.json --signing-rotate-from validation-dev-key --signing-rotate-to validation-dev-key-v2 --signing-rotate-provider deterministic-dev --signing-rotate-algorithm sha256-dev-detached --signing-attest-key-id validation-dev-key-v2 --signing-attestor lab-attestor --signing-attestation-status unverified --signing-attestation-custody-tier software --signing-registry-out artifacts/sim/signing_registry_snapshot.json --out artifacts/sim/integration_registry_lifecycle.json`
9. `cargo run -p pb-integration -- --profile lab --tasks 20 --signer hsm-placeholder --signer-hsm-key-ref local-hsm-ref --signing-registry-in config/signing-registry.example.json --hsm-trust-policy-in config/hsm-trust-policy.example.json --hsm-enforce-trust-policy --out artifacts/sim/integration_hsm_policy.json`
10. `powershell -ExecutionPolicy Bypass -File scripts/finalize-hsm-policy-report.ps1`
11. `cargo run -p pb-integration -- --profile lab --replay-max-mismatch-rate 0.0 --replay-policy-chain-in artifacts/sim/policy_chain.json --replay-kernel-chain-in artifacts/sim/kernel_handoff_chain.json --replay-inputs-in artifacts/sim/integration_inputs.json --replay-consistency-out artifacts/sim/replay_consistency.json`
12. `powershell -ExecutionPolicy Bypass -File scripts/finalize-replay-trend-report.ps1`
13. `cargo run -p pb-safety -- --profile lab --steps 30 --out artifacts/sim/safety.json`
14. `cargo run -p pb-dashboard -- --profile lab --scenario burst_load`

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
