# Reproducible build of `zpl-kernel-bin`

## Goal

The bare-metal kernel binary at
`target/x86_64-zpl-kernel/release/zpl-kernel-bin` should hash to the
same SHA256 across:

1. Repeated builds on the same host (covered by
   `scripts/repro-build.ps1` and CI).
2. Builds on different developer hosts running the same pinned nightly
   toolchain (`rust-toolchain.toml`) and the same workspace commit
   (cross-machine; opt-in second host required).

Goal #1 is what we automate today. Goal #2 needs a second host volunteer
and is tracked under the milestone "v0.7.0 = M11-done".

## How to run locally (same-machine)

```powershell
powershell -ExecutionPolicy Bypass -File scripts/repro-build.ps1 -Rounds 2
```

The script:

1. Cleans the kernel + kernel-bin crates.
2. Rebuilds with the pinned `-Zbuild-std=core,compiler_builtins,alloc`
   flag set against `crates/zpl-kernel/x86_64-zpl-kernel.json`.
3. Computes SHA256 over the resulting `zpl-kernel-bin`.
4. Repeats for `--Rounds N` and asserts every hash matches the first.

A successful run logs:

```
==> repro-build PASS (same-machine deterministic): <sha256>
```

The hashes are also captured in
`artifacts/repro/repro-<timestamp>.log` for retroactive auditing.

## Cross-machine procedure (manual today)

1. Two volunteers check out the same commit on Rust nightly pinned by
   `rust-toolchain.toml`.
2. Each runs `scripts/repro-build.ps1 -Rounds 2`.
3. They exchange the resulting SHA256 via a side channel.
4. If the hashes match, the kernel build is fully reproducible at this
   commit. The hash is then recorded in the release notes for that
   milestone tag.

## Known sources of drift to watch for

- Embedded panic file paths: today we still bake absolute crate paths
  into `core::panic::PanicInfo::location()`. We mitigate by passing
  `--remap-path-prefix` on the kernel build (planned, not yet wired).
- LTO / parallel codegen: the workspace stays on the default release
  profile. Setting `codegen-units = 1` would make builds more
  deterministic but slower; we revisit during Luna 6 hardening.
- `compiler_builtins` features: we explicitly pin
  `compiler-builtins-mem` to keep the same set of intrinsics included.

## Acceptance gate

`scripts/repro-build.ps1` PASSING (exit 0) is the same-machine gate.
Cross-machine matching is a manual procedure logged in the release
notes, deferred until task `#39 GitHub repo public` is approved.
