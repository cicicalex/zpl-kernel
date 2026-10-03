# Verification Pipeline (Task A0 + extensions)

This pipeline is the safety net for changes in `zpl-kernel-lab`.

## Order of gates (each fails fast)

1. `cargo check --workspace`
2. `cargo clippy --workspace -- -D warnings`
3. `cargo test --workspace --target x86_64-pc-windows-msvc --exclude zpl-kernel-bin --exclude limine-bridge`  
   (includes **`zpl-kernel`** host `phys_hhdm` tests; excludes bare-metal
   `zpl-kernel-bin` and `limine-bridge`, which are bare-metal-only targets.)
4. `cargo -Zjson-target-spec build -p zpl-kernel-bin --release`
5. ELF gate (`llvm-objdump -f` + `llvm-readobj --file-headers`)
6. QEMU smoke (`scripts/qemu-boot-test.sh` if `bash` available, else
   PowerShell fallback in `scripts/full-verify.ps1`). Confirms the
   following markers in the COM1 log:
   - `[ZPL-BOOT] kernel_entry reached`
   - `[ZPL-BOOT] serial initialized`
   - `[ZPL-FRAME] init base=0x200000 cap=262144`
   - `[ZPL-FRAME] burst alloc=1000 free=1000 leak=0`
   - `[ZPL-FRAME] stress=1000000 leak=0`
   - `[ZPL-HEAP] selfcheck box+vec100 OK`
   - `[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK`
   - `[ZPL-BOOT] halt loop entered`
7. Determinism (`scripts/determinism-test.sh` or PowerShell fallback)
8. Demo-policy reference (`scripts/demo-policy-test.sh`)
9. `scripts/build-examples.ps1` (every `examples/programs/*.zpla`
   assembles cleanly through `zpl-asm`)
10. `scripts/replay-test.ps1` (audit-replay fixtures: clean PASS,
    tampered FAIL detected)
11. `scripts/fs-test.ps1` (zpl-fs cargo tests + mkfs format/init-audit/
    info + audit-replay store-fs/verify-fs end-to-end)
12. **`scripts/build-iso.ps1`** (Limine hybrid ISO + optional QEMU CD
    smoke) when running `scripts/full-verify.ps1` **without**
    `-SkipLimineIso`; requires xorriso/WSL per [`tools/xorriso/README.md`](../tools/xorriso/README.md).
    See [`PHYSICAL_TEST_GUIDE.md`](./PHYSICAL_TEST_GUIDE.md).

## Entry points

- Local orchestrator: `scripts/full-verify.ps1`
- CI: `.github/workflows/ci.yml`
- Local hook: `.githooks/pre-commit` (the cheap steps only;
  full-verify stays voluntary on the developer side because of QEMU cost).
  Hooks in a repository are inert until a clone is told to use them:
  `git config core.hooksPath .githooks`.

## When to run what

- Run `scripts/full-verify.ps1` before a large change, and before tagging.
- The local hook stops the commit if `cargo check` or `cargo clippy -D warnings`
  fails, and also if the bare-metal build produces a warning -- clippy builds
  for the host and does not cover the kernel's own target.

## Adding a new gate

When introducing a new acceptance check:

1. Drop the script in `scripts/` (PowerShell preferred for the
   Windows-first workflow; provide a `bash` equivalent if the gate
   should also run from non-Windows CI machines).
2. Wire it into `scripts/full-verify.ps1` via `Run-Step`.
3. Mirror the marker check or success line in this document under the
   "Order of gates" list above.
4. If the gate produces artifacts (logs, images, JSON dumps), add the
   directory to `.gitignore` so the working tree stays clean.

The kernel-side markers above are the canonical "did we boot" check.
Adding a marker means updating both the kernel emitter (`boot.rs`),
the PowerShell fallback in `scripts/full-verify.ps1`, the bash gate
in `scripts/qemu-boot-test.sh`, and the assertion list in
`scripts/build-all.ps1`.

## Standalone gates (not in the pre-commit hook, run by hand)

### `scripts/cpuid-clobber-check.ps1`

Reads the **disassembly** of the built bare-metal binary and looks for a
`cpuid` whose EBX result is discarded before it is read.

Added 27 September 2026, after exactly that bug shipped for months. Every
existing gate passed on it: the source compiles, clippy is silent, the
host tests could not reach the module, the boot printed a plausible number
followed by `OK`, and the determinism check was happy because a stale
register is perfectly stable. The full account is in
`docs/V04_TEST_RECORD.md` section 2.2.

```powershell
scripts\cpuid-clobber-check.ps1                                   # built artifact
scripts\cpuid-clobber-check.ps1 -Binary path\to\elf
scripts\cpuid-clobber-check.ps1 -Disassembly dump.txt -Verbose
```

Exit codes: `0` no suspect site, `1` suspect site found, `2` could not run
(missing binary or no `llvm-objdump`) — which is deliberately **not** a
pass.

**Not wired into the pre-commit hook**, because it needs a built
bare-metal artifact and the hook does not produce one. Run it after a
release build, or add it next to the ELF gate in `build-all.ps1` if the
extra minute is acceptable.

**It has a positive control, and the control matters more than usual.**
`tests/fixtures/cpuid-clobber-prefix.txt` is a verbatim window from the
disassembly of the binary that actually had the bug, and the gate must
exit 1 on it. A fixture is used rather than a rebuild because **rebuilding
the broken source does not reliably reproduce the bug**: the same faulty
inline assembly, rebuilt on 27 September, allocated a different register
and came out correct. That is the whole argument for checking the artifact
instead of the source, and it is why the only trustworthy control is the
artifact that failed.

Verified both ways on 27 September 2026: `PASS` (exit 0) on the fixed
binary, `E_CPUID_010_CLOBBER` (exit 1) on the fixture, exit 2 on a missing
path.

**What it does not cover.** One instruction pattern, at `cpuid` sites
only. It says nothing about any other hand-written inline assembly in the
tree, and the general lesson — a constraint that lets the allocator pick a
reserved register — has no gate at all. The durable fix for that class is
to prefer `core::arch::x86_64` intrinsics over hand-rolled `asm!`.
