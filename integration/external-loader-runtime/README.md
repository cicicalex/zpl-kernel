# zpl-external-loader-runtime

Buildable `no_std` crate for external boot stage integration with `zpl-kernel`.

## API

- `RuntimeSnapshot`
  - carries runtime-discovered memory, CPU count, and memory map entry count.
- `handoff_from_snapshot(snapshot, kind)`
  - normalizes snapshot into the selected handoff mode and transfers control.
- `bindings::*`
  - C-compatible runtime handoff structs and pointer-to-snapshot adapters for multiboot2/stivale2 style stages.
- `bindings::RuntimeMemoryMapSource`
  - trait boundary for real boot-stage memory-map extraction.
- `bindings::RuntimeMemoryMapSummary`
  - normalized summary used to build versioned runtime metadata.
- `bindings::RuntimeMemoryMapV1`
  - C ABI memory-map pointer envelope with versioned header and region list pointer.
- `bindings::NativeRuntimeInfoV1`
  - native-v1 C-compatible metadata frame for direct handoff.
- `handoff_from_memory_map_summary(summary, kind)`
  - helper to bridge validated summary data into kernel handoff flow.
- `zpl_external_handoff_memory_map_v1(...)`
  - C ABI entrypoint that builds summary directly from region pointer + count.
- `zpl_external_handoff_multiboot2_v1(...)`
- `zpl_external_handoff_stivale2_v1(...)`
- `zpl_external_handoff_native_v1(...)`
  - C ABI handoff entrypoints for direct external stage integration.
- mode constants:
  - `HANDOFF_MODE_NATIVE_V1`
  - `HANDOFF_MODE_MULTIBOOT2`
  - `HANDOFF_MODE_STIVALE2`
  - use these instead of numeric literals in external stage integrations.

## Integration

1. Read real runtime values in your boot stage.
2. Populate v1 runtime metadata (`protocol_magic`, `protocol_version`, memory/cpu/map counters).
3. Build `RuntimeSnapshot` from validated values.
4. Call `handoff_from_snapshot(...)` with desired `BootStackKind`.

For strict stage paths that already have versioned runtime metadata, you can call
`zpl_external_handoff_multiboot2_v1(...)` / `zpl_external_handoff_stivale2_v1(...)` directly.
Template helpers now also enforce the same runtime upper bounds used by the runtime crate
(`cpu_count <= 4096`, `memory_map_entries <= 4096`) before attempting handoff.

### Stage wiring checklist (per boot stack)

- `native-v1` path:
  1. Build `NativeRuntimeInfoV1` with valid protocol header and non-zero counters.
  2. Call `zpl_external_handoff_native_v1(ptr, HANDOFF_MODE_NATIVE_V1)`.
- `multiboot2` path:
  1. Build `Multiboot2RuntimeInfoV1` from your stage parser output.
  2. Call `zpl_external_handoff_multiboot2_v1(ptr, HANDOFF_MODE_MULTIBOOT2)` (or other explicit mode if required by your handoff strategy).
- `stivale2` path:
  1. Build `Stivale2RuntimeInfoV1` from your stage parser output.
  2. Call `zpl_external_handoff_stivale2_v1(ptr, HANDOFF_MODE_STIVALE2)` (or other explicit mode if required by your handoff strategy).
- memory-map envelope path:
  1. Build `RuntimeMemoryMapV1` + region list pointer with valid `region_count`.
  2. Call `zpl_external_handoff_memory_map_v1(ptr, HANDOFF_MODE_*)`.
  3. Ensure each region passes raw validation (`length > 0`, no `base + length` overflow).

## Runtime metadata contract

- `protocol_magic = 0x5A504C52` (`"ZPLR"`)
- `protocol_version = 1`
- `total_memory_bytes > 0`
- `cpu_count > 0`
- `memory_map_entries > 0`
- `cpu_count <= 4096`
- `memory_map_entries <= 4096`
- `region_count <= 4096` for `RuntimeMemoryMapV1`
- each memory region must satisfy `base + length` without overflow

Invalid metadata is rejected fail-closed by the C ABI entrypoints.

`RuntimeMemoryMapV1` accepts regions tagged by `MEMORY_REGION_USABLE = 1`; total memory is
computed as the sum of usable region lengths.
