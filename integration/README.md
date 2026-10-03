# External Loader Integration Template

This folder contains handoff templates for integrating a real bootloader/stage project with `zpl-kernel`.

## File

- `external-loader-runtime/`
  - Buildable `no_std` crate for external stage runtime integration.
  - Implements `ExternalBootRuntime` with runtime-driven snapshot API.
- `external-loader-runtime-template.rs`
  - Implements `ExternalBootRuntime`
  - Publishes stable handoff pointers
  - Calls `enter_from_external_runtime(...)`
- `external-loader-multiboot2-strict-template.rs`
  - Strict one-path template for Multiboot2 stage using runtime input struct.
- `external-loader-stivale2-strict-template.rs`
  - Strict one-path template for Stivale2 stage using runtime input struct.

## How to use

1. Prefer using `external-loader-runtime/` directly in the external stage build.
2. If using templates, copy template file into boot stage repository.
3. Feed real runtime snapshot values (memory/cpu/map) into template input structs.
4. Keep the published slot pointer alive until kernel entrypoint reads it.
5. Call the helper with selected mode (`NativeV1`, `Multiboot2`, or `Stivale2`).
