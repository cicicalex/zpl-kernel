# `examples/` — what's here, what runs them

This directory contains the third-party-style demos referenced
elsewhere in the repo. Files are split by demo type, not by
language, so the boundaries are clean even if a demo is multi-step.

## `programs/` — `.zpla` source files

Compiled by `crates/zpl-asm/` into bare-metal ELFs that run under
the kernel ELF loader (`crates/zpl-kernel/src/fs/elf.rs`). The
short version:

| File | Demonstrates | Gate |
|------|-------------|------|
| `hello_ain.zpla` | `SYS_LOG("hi")` + `SYS_EXIT(0)` | `scripts/elf-test.ps1` |
| `bias_sweep.zpla` | Two `SYS_LOG` calls | `scripts/build-examples.ps1` |
| `policy_test.zpla` | Single `SYS_LOG` + `SYS_EXIT` | same |
| `audit_dump.zpla` | `SYS_READ_AUDIT` stub | same |
| `matrix_op.zpla` | Bitwise XOR/AND/OR + `SYS_LOG` | same |
| `attack.zpla` | Demo killer: `SYS_COMPUTE` with bias=0.95 -> kernel BLOCK | `scripts/attack-test.ps1` |

Build all of them:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build-examples.ps1
```

## `c/` — C-language demos (deferred)

`hello.c` mirrors `hello_ain.zpla` against the hand-rolled
a hand-written C header for the same ABI. It compiles once a GCC
ring-3 toolchain for this ABI exists; until then the assembly
version is the canonical demo.

## `traces/` — synthetic packet traces for `zpl-firewall-cli`

| File | Distribution | Gate |
|------|--------------|------|
| `clean.csv` | 5 packets, all ALLOW | the firewall test, step 2 |
| `attack.csv` | 5 port-scan packets, all DROP | step 3 |
| `mixed.csv` | 8 packets sweeping ALLOW + DEGRADE + DROP (3+3+2) | step 4 |

Run any of them by hand:

```powershell
cargo run -q -p zpl-firewall-cli -- examples/traces/clean.csv
cargo run -q -p zpl-firewall-cli -- examples/traces/attack.csv --audit-output artifacts/audit/firewall-attack.json
cargo run -q -p audit-replay -- summary artifacts/audit/firewall-attack.json
```

## `crates/pb-core/examples/` — research crate demos

Lower-level reference implementations exercised by the existing
`pb-core` test suite:

| File | Purpose |
|------|---------|
| `zpl_policy_userspace_ref.rs` | Reference user-space mirror of the kernel `zpl_policy` decision boundary. |
| `zpl_policy_bit_identical.rs` | Bit-identity proof between host and kernel. |
| `audit_hash_diag.rs` | Audit-chain hash-mixer diagnostic. |

## See also

- [`docs/MARKERS.md`](../docs/MARKERS.md) — every `[ZPL-...]` marker the kernel emits.
- `scripts/demo-day.ps1` — one-shot launch capture covering every
  scenario in `programs/`, `traces/`, the kernel boot smoke, and
  the reproducible-build hash.
