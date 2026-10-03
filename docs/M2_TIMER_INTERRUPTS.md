# M2 — Timer interrupts wiring

## Goal
Kernel receives PIT IRQ0 timer interrupts and emits `[TICK]` lines on COM1.

## Architecture

PVH delivers control in 32-bit protected mode. `_start` (assembly in
`crates/zpl-kernel/src/start.rs`) builds a 4-level identity-mapped page
table for the first 1 GiB (PML4 -> PDPT -> PD with 2 MiB pages), enables
`CR4.PAE`, sets `EFER.LME`, sets `CR0.PG`, loads a 64-bit GDT, and far-jumps
to 64-bit code (`zpl_long_mode_entry`). Then x87 + SSE are enabled
(`CR0.EM=0`, `CR0.MP=1`, `CR4.OSFXSR=1`, `CR4.OSXMMEXCPT=1`) so the rest
of the kernel can use `f64`.

`crates/zpl-kernel/src/interrupts.rs` provides:

- 256-entry IDT with 16-byte long-mode entries (`IdtEntry`).
- Legacy 8259 PIC remapped to vectors 0x20 (master) and 0x28 (slave).
- PIT channel 0 reprogrammed via ports 0x40/0x43 for ~100 Hz cadence.
- `extern "x86-interrupt" fn timer_isr(_frame: InterruptStackFrame)` at
  vector 32 = IRQ0 after remap.
- ISR writes `[TICK]\n` to COM1 (port 0x3F8) with TX-ready polling on LSR
  bit 5, then sends EOI = `0x20` to PIC master `0x20`.

`init()` is called from `kernel_entry()` after the boot serial markers.
After `sti`, IRQ0 fires and `[TICK]` lines stream out.

## Acceptance

- `cargo build -p zpl-kernel-bin --release` clean.
- QEMU 3 s run produces:
  - 3 boot markers
  - >= 100 lines `[TICK]`
  - 0 triple fault in `-d int,cpu_reset,guest_errors` trace.

## Reference run

`artifacts/kernel/m2-clean.log`: 3 boot markers + 1786 `[TICK]` over 3 s
(~595 lines/s effective; PIT target was ~100 Hz; QEMU CPU is faster).

`artifacts/kernel/m2-rerun-debug.log`: 0 `Triple fault` in 3 s window.
