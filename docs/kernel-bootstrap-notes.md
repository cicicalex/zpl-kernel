# Kernel Bootstrap Notes

## Current scaffold

- `zpl-kernel/src/start.rs` defines `_start`.
- `zpl-kernel/src/boot.rs` exposes `kernel_entry`.
  - includes serial boot-log scaffold (`zpl-kernel/src/drivers/serial.rs`) for deterministic boot context logging.
  - boot path now tags log frames with detected x86_64 source id (`CPUID` APIC id byte).
  - boot path now routes secondary-source orchestration through `zpl-kernel/src/ap_lifecycle.rs`.
  - AP lifecycle module now exposes explicit registration APIs for real startup/teardown hooks.
  - AP events are now staged through `zpl-kernel/src/ap_hooks.rs` (queue + event adapter) and drained through a single handler path.
  - AP hook source contract available via `ApEventSource`; boot loop can run in `ExternalHooksOnly` mode to disable deterministic synthetic AP activity.
- `zpl-kernel/src/mm/boot_stack.rs` provides concrete handoff adapters:
  - `zpl_boot_entry_v1`
  - `zpl_boot_entry_multiboot2_v1`
  - `zpl_boot_entry_stivale2_v1`
  - all external handoff entrypoints are fail-closed on invalid pointers/frames (no minimal boot fallback).
- `zpl-kernel/src/loader_stage.rs` provides loader lifecycle contract:
  - publish handoff frame in static slot
  - pass stable pointer to matching entrypoint
  - transfer control through `handoff_and_enter`
  - external runtime trait `ExternalBootRuntime`
  - helper `enter_from_external_runtime(...)` for deterministic wiring
- `integration/external-loader-runtime/src/bindings.rs` now defines versioned runtime metadata contract:
  - `protocol_magic = 0x5A504C52` (`ZPLR`)
  - `protocol_version = 1`
  - fail-closed validation before handoff
  - summary boundary helpers for real-stage extraction:
    - `RuntimeMemoryMapSource`
    - `RuntimeMemoryMapSummary`
    - `*_info_from_summary(...)`
- `integration/external-loader-runtime/src/lib.rs` now exposes:
  - `zpl_external_handoff_native_v1`
  - `zpl_external_handoff_multiboot2_v1`
  - `zpl_external_handoff_stivale2_v1`
  - `zpl_external_handoff_memory_map_v1`
  with strict mode parsing and invalid-input halt behavior.
- `zpl-kernel/src/drivers/serial.rs` now supports sink mode:
  - ring buffer only (`BufferOnly`)
  - ring buffer + COM1 mirror on bare-metal x86_64 (`MirrorCom1`)
  - ring buffer + COM1 + VGA mirror (`MirrorCom1AndVga`)
  - ring write strategy mode (`BootLogRingWriteMode`):
    - `GlobalLock`
    - `LockFreeBestEffort`
    - `SourceShardedLockFree`
  - bare-metal path defaults ring writes to source-sharded lock-free mode
  - compact code labels and sink-drop telemetry (`LOG_BOOT_SINK_DROP_COUNT`)
  - bounded ring-lock telemetry (`LOG_BOOT_RING_DROP_COUNT`) with tunable spin budget
  - ring overwrite telemetry (`LOG_BOOT_RING_OVERWRITE_COUNT`) for wrap-pressure visibility
  - lane-tagged entries (`log_boot_entry_on_lane`) for separating context/heartbeat/health channels
  - source-tagged and flags-tagged frames via `log_boot_entry_on_lane_from_source(...)`
  - source context API available (`set_current_boot_source_id` / `current_boot_source_id`) for AP-path log routing
  - source-channel occupancy introspection (`snapshot_source_channel_occupancy(...)`)
  - source-focused snapshots available (`snapshot_boot_log_for_source*`)
  - source lifecycle registry available (`set_boot_source_online`, `snapshot_boot_sources_online`, `boot_source_online_count`)
  - sink-health stream now includes source-online aggregate counter (`LOG_BOOT_SOURCE_ONLINE_COUNT`)
  - structured frame schema v1 available for consumers (`BootLogFrameV1`, `snapshot_boot_log_frames_v1`)
  - AP lifecycle helpers available (`mark_current_boot_source_online/offline`) and source-heartbeat metrics (`record_current_boot_source_heartbeat`, `snapshot_boot_source_metrics`)
  - boot loop now includes deterministic secondary-source orchestration based on `cpu_count` (online + heartbeat + rotating lifecycle state)
  - sink lane routing mask (`set_sink_lane_mask`, `set_sink_lane_enabled`) to reduce mirrored traffic without dropping ring-buffer evidence
  - per-lane ring extraction helper (`snapshot_boot_log_for_lane`) for focused diagnostics
  - code/lane combined ring extraction helpers (`snapshot_boot_log_for_code`, `snapshot_boot_log_for_lane_and_code`)
  - sink lane presets (`apply_sink_lane_preset`) for fast profile switches (all/context/context+health/context+heartbeat)
  - monotonic per-entry sequence ids for deterministic ordering/gap detection
  - serial state snapshot/reset helpers (`snapshot_boot_log_stats`, `reset_boot_log_state`, `reset_boot_log_sink_mode`) for deterministic reruns
- `zpl-kernel/linker.ld` provides minimal section layout.
- `zpl-kernel/x86_64-zpl-kernel.json` defines custom target.
- `zpl-kernel/.cargo/config.toml` wires target + linker flags.

## Next boot tasks

1. Emit tick logs on timer interrupts.
2. Connect scheduler input from synthetic task queue.
3. Validate AP lifecycle producer behavior under sustained VM multiprocessor pressure (heartbeat/online-offline drift checks).
4. (done 2026-04-20) Replay signed-chain integrity parity now green across 3/3 reruns; fixed via `serde_json` `float_roundtrip` feature.
5. Harden the audit hashing surface against future serializer-induced drift (consider using a canonical JSON form of each `AuditEvent` as the hash input, or storing explicit `score_bits` hex alongside the `score` number).
