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

## Linux and CI: kernel, Limine ELF and ISO

CI checks three things on every push, with scripts you can run yourself:

| Script | What it proves |
|---|---|
| `scripts/build-iso.sh` | Builds the published ISO on Linux: same pinned Limine release and checksum, staging, `limine.conf` and xorriso arguments as `build-iso.ps1`. |
| `scripts/repro-check.sh` | Checks out the commit twice, in two directories with different path lengths, builds each from clean, and compares the SHA-256 of the `-kernel` ELF, the Limine ELF and the ISO. Any difference fails. |
| `scripts/path-gate-check.sh` | Scans the built bytes for build-machine paths. See "No build paths in the binaries" below. |

```bash
bash scripts/build-iso.sh            # artifacts/iso/zpl-kernel.iso (+ .sha256)
bash scripts/repro-check.sh          # two clean builds, compared
bash scripts/path-gate-check.sh      # the gate, and the proof it can fail
```

Measured when this was written: all three outputs byte-identical between two
clean builds from `/tmp/zpl-repro-check/a` and
`/tmp/zpl-repro-check/second-checkout-with-a-longer-path`. A third build, from the
developer checkout itself, matched them too (with the same `SOURCE_DATE_EPOCH`).

### What makes the ISO reproducible, and what still differs

- **Timestamps.** Every date in the image comes from `SOURCE_DATE_EPOCH`. By
  default it is the commit time (`git log -1 --format=%ct`). The staged files are
  dated to it, and xorriso honours the variable for the volume dates. **So the ISO's
  hash depends on the commit time by design**: two commits with the same tree but
  different times give different ISOs. The two ELFs do not depend on it.
- **MBR disk ID.** `limine bios-install` writes four random bytes at offset `0x1B8`,
  seeded from the clock (`srand(time(NULL))` in `limine.c`). Before this was handled,
  two builds a second apart differed in exactly those four bytes and nowhere else.
  `build-iso.sh` now overwrites them with the first four bytes of the SHA-256 of the
  image with those bytes zeroed. The ID still only has to tell disks apart.
- **Still different: `build-iso.ps1` on Windows.** It does not set the dates or the
  disk ID yet, so two Windows builds of one commit still differ. Build a release ISO
  with `build-iso.sh`, or at the least run the path gate on the Windows ISO before
  publishing it: `python scripts/path-leak-gate.py artifacts/iso/zpl-kernel.iso`.
- **Across machines:** not measured yet. It needs the same nightly toolchain
  (`rust-toolchain.toml` names a channel, not a date) and the same xorriso version.
  The procedure below still applies.

## No build paths in the binaries

A Windows account's `.rustup` folder once travelled inside every public ISO: a
`panic!` in the standard library records the file it came from, and with
`-Zbuild-std` that file is on the build machine.

`trim-paths = "all"` in the workspace `Cargo.toml` is what stops it. It is
implemented by cargo as `--remap-path-prefix` flags for the workspace, the build
directory, the registry and the standard library sources under `.rustup`, together
with `--remap-path-scope=all`, which covers debug info and macro output as well.
`cargo build -v` shows them. The prefixes are written at build time from the machine
that builds. The alternative of listing them by hand in a published file would
publish that machine's paths, the same leak in a new place. The assembly
(`global_asm!`) goes through rustc and is covered by the same flags. The linker
(rust-lld) takes the linker script by a relative path and records no directory.

`scripts/path-leak-gate.py` checks the result in the built bytes: it fails on
`/home/`, `/root/`, `C:\Users\`, `C:/Users/`, `.rustup` or `.cargo`, matching
regardless of case, as ASCII and as UTF-16 (both byte orders). `scripts/path-gate-check.sh`
runs it four ways, so a pass means something:

1. strings planted into a copy of the kernel, every mark in every encoding: all caught;
2. the kernel built again with `trim-paths` off, which is the incident reproduced:
   rejected (it carries `<home>/.rustup/toolchains/...`);
3. a debug build with full DWARF: clean;
4. the release kernel ELF, the Limine ELF and the ISO: clean.

## How to run locally on Windows (same-machine)

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

- Embedded panic file paths: handled by `trim-paths = "all"` (see "No build
  paths in the binaries" above), and checked in the built bytes by CI.
- LTO / parallel codegen: the workspace stays on the default release
  profile. Setting `codegen-units = 1` would make builds more
  deterministic but slower; we revisit during Luna 6 hardening.
- `compiler_builtins` features: we explicitly pin
  `compiler-builtins-mem` to keep the same set of intrinsics included.

## Acceptance gate

`scripts/repro-check.sh` passing in CI is the same-machine gate for the
kernel, the Limine ELF and the ISO. `scripts/repro-build.ps1` (exit 0) is the
Windows equivalent for the kernel ELF.
Cross-machine matching is a manual procedure logged in the release
notes, deferred until task `#39 GitHub repo public` is approved.
