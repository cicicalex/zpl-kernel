# ZPL Kernel COM1 Marker Catalogue

Single source of truth for every `[ZPL-...]` marker the kernel
emits. Used by gate scripts, the test runner, demo-day captures, and
the press kit. When you add a new marker, update this file and
extend `scripts/full-verify.ps1` + `scripts/build-all.ps1` +
`scripts/qemu-boot-test.sh` so the verification fortress sees it.

## ISO vs QEMU `-kernel` (Limine)

The markers below are what you should see on **COM1** once the kernel
reaches `kernel_entry`. The **QEMU `-kernel` Multiboot-style** path still
uses `zpl-kernel-bin` / `_zpl_qemu_boot`. The **Limine hybrid ISO**
path uses `limine-bridge` and then the same entrypoint — see
[`PHYSICAL_TEST_GUIDE.md`](./PHYSICAL_TEST_GUIDE.md). For where
`build-iso.ps1` and related scripts sit in the automation map, see
`scripts/full-verify.ps1`, which runs the scripted gates.

## Boot lifecycle (always emitted)

| Marker | Source | Meaning |
|--------|--------|---------|
| `[ZPL-BOOT] kernel_entry reached` | `boot.rs` | First instruction after long-mode bootstrap. |
| `[ZPL-BOOT] serial initialized` | `boot.rs` | COM1 polling driver wired up. |
| `[ZPL-BOOT] halt loop entered` | `boot.rs` | All self-checks done; kernel parks in `halt_once` loop. |
| `[ZPL-EVT2 tag=BUILD-ID t=0 kind=kernel_image version=v0.5.0]` | `trace_event.rs` | Structured build identifier. |

## Subsystem self-checks

These run unconditionally in the default-feature build, in this
order:

| Marker | Source | Self-check |
|--------|--------|-----------|
| `[ZPL-FRAME] init base=0x00200000 cap=N` | `frame_alloc` | Bitmap allocator initialised at 2 MiB physical. `N` is how many frames this machine actually offers, after the firmware memory map is applied, so it differs between machines and between QEMU `-m` sizes. On a boot path with no memory map (the `-kernel` build) it is the whole bitmap, 262144. |
| `[ZPL-FRAME] burst alloc=1000 free=1000 leak=0` | same | 1 000 alloc/free round-trip. |
| `[ZPL-FRAME] stress=1000000 leak=0` | same | 1 M iterations of grow/shrink. |
| `[ZPL-HEAP] selfcheck box+vec100 OK` | `heap::self_check` | `Box::new(0xCAFE_BABE)` + 100-element `Vec` round-trip. |
| `[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK` | `audit_chain::self_check` | 5 records, chain re-hash matches, tampered byte detected. |
| `[ZPL-SMP max_leaf=N max_logical=M apic_id=X OK]` | `smp::probe` | CPUID 0x00 + CPUID 0x01 read; QEMU 1-vCPU reports `max_leaf=13 max_logical=0 apic_id=0`. This row described the intended values; until the CPUID fix of 26 Sep 2026 the kernel actually printed `max_logical=17/18 apic_id=128`, a stale register rather than hardware (`V04_TEST_RECORD.md` §2.2). It now prints what this row says. |
| `[ZPL-PAGING] init pdpt[1]+pd[0] ok` | `paging::self_check_*` | PDPT entry wired to PD; first PD slot clean. |
| `[ZPL-PAGING] map_4k virt=0x40000000 ok` | same | A 4 KiB page mapped into the user range. |
| `[ZPL-PAGING] write_read_match val=0xcafebabedeadbeef` | same | Write-then-read round-trip via the new mapping. |
| `[ZPL-PAGING] unmap virt=0x40000000 ok` | same | Same page unmapped + `invlpg`. |
| `[ZPL-PAGING] page_fault virt=... reread ok` | `paging::handle_pf` | Synthetic #PF demonstrates the kernel handler. |
| `[ZPL-RING] tss installed selector=0x28` | `tss::init` | TSS loaded, GDT entry patched. |
| `[ZPL-STACKGUARD] install body=8KiB guard=0x40004000 ok` | `stack_guard` | Stack canary frame installed. |
| `[ZPL-STACKGUARD] body_write_read ok` | same | Body of the guarded stack is writable. |
| `[ZPL-STACKGUARD] guard_hit virt=... cleanly handled` | same | The guard page traps a write attempt. |
| `[ZPL-RAMFS] selfcheck create+write+close+reopen+read OK` | `ramfs::self_check` | RAM filesystem round-trip. |
| `[ZPL-PROC] selfcheck spawn=3 wait=3 exit_codes_sum=43 live=0 OK` | `process::self_check` | In-kernel process table. |
| `[ZPL-IPC] selfcheck pipe+send+recv ipc-roundtrip OK` | `ipc::self_check` | Ring-buffer pipes. |
| `[ZPL-SHM] shm_id=0 ain=99 action=ALLOW` | `shm::self_check` | AIN-gated shared memory: clean write. |
| `[ZPL-SHM] shm_id=0 ain=14 action=BLOCK` | same | AIN-gated shared memory: hostile write rejected. |
| `[ZPL-SHM] selfcheck allow_clean+block_hostile OK` | same | Final OK marker after both transitions. |
| `[ZPL-MATRIX] selfcheck 8ops naive==simd 64x64 OK` | `matrix_ops::self_check` | All 8 operators agree between naive and `__m128i` paths. |
| `[ZPL-RUNQ] selfcheck N=5 ticks=10 winner=task0_wins=10 OK` | `runqueue::self_check` | AIN-priority scheduler picks the right task across 10 ticks. |
| `[ZPL-HAL] selfcheck block_rw+net_tx_rx OK` | `hal::self_check` | Block + net mock devices. |
| `[ZPL-VNET] probe slot=N …` | `boot::run_virtio_net_probe` | PCI scan + BAR0; legacy I/O adds `io=0x… st=0x…`, else `io=skip`. |

## Scheduler trace (continuous)

| Marker | Source | Meaning |
|--------|--------|---------|
| `[ZPL-SCHED tid=A\|B\|C ain=NN action=ALLOW\|DEGRADE\|BLOCK]` | timer ISR | One line per timer tick once the scheduler is live. |

## Performance profiler (always emitted at boot end)

| Marker | Source | Meaning |
|--------|--------|---------|
| `[ZPL-PERF phase=N name=... rdtsc=... delta=...]` | `timing::emit` | Per-phase rdtsc snapshot (16 phases). |

## Feature-gated demo flows

These appear only when the matching cargo feature is on (the
default build does *not* include them so determinism baselines stay
stable).

### `panic_test` feature

| Marker | Meaning |
|--------|---------|
| `[ZPL-PANIC] panic in kernel_entry` | Panic handler ran. |
| `[ZPL-RING] qemu_exit=success` | `isa-debug-exit` returns code 1 (success in the panic-test gate). |

### `ring3_demo` feature

| Marker | Meaning |
|--------|---------|
| `[ZPL-RING] hand-rolled program entered` | iretq into ring 3 succeeded. |
| `[ZPL-SYSCALL] log: hi` | User program made `SYS_LOG`. |
| `[ZPL-SYSCALL] exit code=0` | User program made `SYS_EXIT(0)`. |
| `[ZPL-RING] qemu_exit=success` | Test gate happy. |

### `elf_demo` feature

| Marker | Meaning |
|--------|---------|
| `[ZPL-ELF] demo entered` | Kernel about to invoke the embedded ELF. |
| `[ZPL-ELF] load ok` | ELF parser succeeded. |
| `[ZPL-ELF] entry=0x...` | Entry point chosen. |
| `[ZPL-SYSCALL] ...` | Same as ring3 demo. |
| `[ZPL-RING] qemu_exit=success` | Test gate happy. |

### `attack_demo` feature

| Marker | Meaning |
|--------|---------|
| `[ZPL-ELF] demo entered` | Same as elf_demo path. |
| `[ZPL-SCHED tid=user ain=15 action=BLOCK]` | The demo killer fires; AIN drops below the BLOCK threshold and the kernel refuses to commit. |
| `[ZPL-SYSCALL] exit code=0` | User program calls `SYS_EXIT(0)` after the BLOCK so the gate observes a graceful exit. |
| `[ZPL-RING] qemu_exit=success` | Test gate happy (exit code 1 from `isa-debug-exit`). |

### Public `-kernel` build (`--features=public`, no feature flag of its own)

Gated on `qemu_boot` + `vga_crit_mirror`, which the public build turns on for
itself. The Limine / ISO path does **not** produce these: it has its own
on-screen summary instead.

Three small programs run in ring 3 at the end of the boot, one per rule of the
demo policy. Each announces itself with `SYS_LOG` before it asks, so the request
and its answer sit next to each other in the log.

| Marker | Meaning |
|--------|---------|
| `[ZPL-RING3] demo programs: clean, hostile, flood` | About to `iretq` into ring 3. |
| `[ZPL-SYSCALL] log: program 1 (clean): one modest request` | Program 1 says what it will ask for. |
| `[ZPL-SCHED tid=user ain=95 action=ALLOW]` | The gate allows it — rule 1, within the budget. |
| `[ZPL-SYSCALL] log: program 2 (hostile): asks far over the limit` | Program 2 announces itself. |
| `[ZPL-SCHED tid=user ain=5 action=BLOCK]` | Refused — rule 2, over the limit. The program does not then attempt the write. |
| `[ZPL-SYSCALL] log: program 3 (flood): asks 400 times, hits the quota` | Program 3 announces itself. |
| `[ZPL-SCHED tid=user ain=95 action=ALLOW]` ×N, then `ain=30 action=DEGRADE` | The per-boot quota runs out part-way through the loop — rule 3. |
| `[ZPL-SYSCALL] exit code=0` | `SYS_EXIT`. In this build it hands control back to the kernel rather than shutting the machine down. |
| `[ZPL-RING3] demo programs done` | Back in ring 0. |
| `[ZPL-BOOT] halt loop entered` | Emitted from `demo_programs::finish`, not from `kernel_entry`, so it still lands where the boot actually ends. |

**Not in the serial log:** the policy panel on the bottom six rows of the screen.
It writes to the text buffer only and never to COM1, which is what keeps every
hash and gate derived from the serial output unchanged by it. The only thing that
goes the other way — serial but not screen — is the `[ZPL-PERF …]` block. So for
this build: *screen = serial minus the PERF lines*.

## Update workflow

When you add a new self-check:

1. Add the marker definition to the source module.
2. Add a row to the relevant table in this file.
3. Extend the gate scripts (`scripts/full-verify.ps1`,
   `scripts/build-all.ps1`, `scripts/qemu-boot-test.sh`) so the
   marker is asserted on every kernel build.
4. Update `crates/zpl-test-runner/src/main.rs` if the marker is
   part of the `boot` scenario.
5. Note the marker in `docs/VERIFICATION_PIPELINE.md` "Order of
   gates" list so the README quick-start mentions it.
