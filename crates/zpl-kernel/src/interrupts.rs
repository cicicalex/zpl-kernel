//! Minimal interrupt scaffolding for M2.
//!
//! Sets up an IDT with a single handler at vector 32 (legacy IRQ0 after PIC
//! remap), reprograms the legacy 8259 PIC, configures the PIT to fire IRQ0 at
//! ~100Hz, and emits a `[TICK]` line to COM1 from the timer ISR. This module
//! is intentionally self-contained and does not depend on any of the higher
//! level lab abstractions.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::port::{inb, outb};
use crate::zpl_policy::{policy_compute, evaluate, ComputeInput, Decision, PolicyConfig};

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const fn empty() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            zero: 0,
        }
    }

    fn set_handler(&mut self, addr: u64, selector: u16) {
        self.offset_low = (addr & 0xFFFF) as u16;
        self.selector = selector;
        self.ist = 0;
        self.type_attr = 0x8E;
        self.offset_mid = ((addr >> 16) & 0xFFFF) as u16;
        self.offset_high = (addr >> 32) as u32;
        self.zero = 0;
    }

    fn set_handler_dpl(&mut self, addr: u64, selector: u16, dpl: u8) {
        self.set_handler(addr, selector);
        // Replace P|DPL|Type byte: P=1 (0x80) | DPL (bits 5-6) | Type 0xE.
        self.type_attr = 0x8E | ((dpl & 0x3) << 5);
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

const IDT_LEN: usize = 256;
static mut IDT: [IdtEntry; IDT_LEN] = [IdtEntry::empty(); IDT_LEN];
pub static TICK_COUNT: AtomicU64 = AtomicU64::new(0);
pub static SCHED_INDEX: AtomicU64 = AtomicU64::new(0);

const TIMER_VECTOR: usize = 32;
/// IRQ 11, where the PCI network card sits on QEMU's default machine. The
/// slave PIC is remapped to base 0x28 and IRQ 11 is its fourth input, so the
/// vector is 0x28 + 3.
const NIC_VECTOR: usize = 0x2B;
const PAGE_FAULT_VECTOR: usize = 14;
const SYSCALL_VECTOR: usize = 0x80;

#[derive(Clone, Copy)]
pub struct KernelTask {
    pub id: u8,
    pub bias: f64,
    pub seed_offset: u64,
}

const SCHED_TASKS: [KernelTask; 3] = [
    KernelTask { id: b'A', bias: 0.48, seed_offset: 0x11 },
    KernelTask { id: b'B', bias: 0.50, seed_offset: 0x22 },
    KernelTask { id: b'C', bias: 0.52, seed_offset: 0x33 },
];

const POLICY: PolicyConfig = PolicyConfig::pb_core_default();

// --- Early fault IDT (before `init()` full PIC/PIT + `sti`) -----------------

const HEX_L: &[u8; 16] = b"0123456789abcdef";

/// COM1 without waiting for the transmitter: this runs inside an early fault handler,
/// where spinning on a status bit is how a fault turns into a hang.
#[inline]
unsafe fn com1_raw_putc(b: u8) {
    // SAFETY: COM1, and a fault handler is the only thing running.
    unsafe { crate::arch::port::outb(0x3F8, b) }
}

unsafe fn com1_hex_u64_full(v: u64) {
    for i in 0..16 {
        let shift = 60 - i * 4;
        let nib = ((v >> shift) & 0xf) as usize;
        com1_raw_putc(HEX_L[nib]);
    }
}

unsafe fn com1_u8_dec(n: u8) {
    if n >= 100 {
        com1_raw_putc(b'0' + n / 100);
        com1_raw_putc(b'0' + (n / 10) % 10);
        com1_raw_putc(b'0' + n % 10);
    } else if n >= 10 {
        com1_raw_putc(b'0' + n / 10);
        com1_raw_putc(b'0' + n % 10);
    } else {
        com1_raw_putc(b'0' + n);
    }
}

unsafe fn emit_zpl_fault_line(vec: u8, err: u64, rip: u64, cr2: u64) {
    asm!("cli", options(nomem, nostack, preserves_flags));
    for b in b"[ZPL-FAULT] vec=" {
        com1_raw_putc(*b);
    }
    com1_u8_dec(vec);
    for b in b" err=0x" {
        com1_raw_putc(*b);
    }
    com1_hex_u64_full(err);
    for b in b" rip=0x" {
        com1_raw_putc(*b);
    }
    com1_hex_u64_full(rip);
    for b in b" cr2=0x" {
        com1_raw_putc(*b);
    }
    com1_hex_u64_full(cr2);
    com1_raw_putc(b'\n');
}

fn fault_hang() -> ! {
    loop {
        unsafe {
            asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

extern "x86-interrupt" fn early_de(frame: InterruptStackFrame) {
    unsafe {
        emit_zpl_fault_line(0, 0, frame.instruction_pointer, 0);
        fault_hang();
    }
}

extern "x86-interrupt" fn early_ud(frame: InterruptStackFrame) {
    unsafe {
        emit_zpl_fault_line(6, 0, frame.instruction_pointer, 0);
        fault_hang();
    }
}

extern "x86-interrupt" fn early_df(frame: InterruptStackFrame, err: u64) {
    unsafe {
        emit_zpl_fault_line(8, err, frame.instruction_pointer, 0);
        fault_hang();
    }
}

extern "x86-interrupt" fn early_gp(frame: InterruptStackFrame, err: u64) {
    unsafe {
        emit_zpl_fault_line(13, err, frame.instruction_pointer, 0);
        fault_hang();
    }
}

extern "x86-interrupt" fn early_pf(frame: InterruptStackFrame, err: u64) {
    let cr2: u64;
    unsafe {
        asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
        emit_zpl_fault_line(14, err, frame.instruction_pointer, cr2);
        fault_hang();
    }
}

/// Minimal IDT gates for common CPU exceptions **before** `emit_critical_marker` and
/// before [`init`] reprograms vector 14 for the paging demo path.
///
/// # Safety
/// Single-threaded BSP early boot; must run with IF=0. Reloads full `IDT` table via `lidt`.
pub unsafe fn install_early_idt() {
    let cs: u16;
    unsafe {
        asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
        IDT[0].set_handler(early_de as *const () as u64, cs);
        IDT[6].set_handler(early_ud as *const () as u64, cs);
        IDT[8].set_handler(early_df as *const () as u64, cs);
        IDT[13].set_handler(early_gp as *const () as u64, cs);
        IDT[14].set_handler(early_pf as *const () as u64, cs);

        let limit = (core::mem::size_of::<[IdtEntry; IDT_LEN]>() - 1) as u16;
        let base = core::ptr::addr_of!(IDT) as u64;
        let ptr = IdtPointer { limit, base };
        asm!(
            "lidt [{0}]",
            in(reg) &ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

/// Initialize IDT/PIC/PIT and enable interrupts.
///
/// # Safety
/// Caller must ensure this runs exactly once on the bootstrap CPU while
/// interrupts are still disabled. Re-initializing PIC/IDT concurrently or
/// enabling interrupts before this setup is complete is undefined behavior.
pub unsafe fn init() {
    unsafe {
        let cs: u16;
        asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
        IDT[TIMER_VECTOR].set_handler(timer_isr as *const () as usize as u64, cs);
        IDT[PAGE_FAULT_VECTOR].set_handler(page_fault_isr as *const () as usize as u64, cs);
        IDT[NIC_VECTOR].set_handler(nic_isr as *const () as usize as u64, cs);
        IDT[SYSCALL_VECTOR].set_handler_dpl(syscall_isr as *const () as usize as u64, cs, 3);

        let limit = (core::mem::size_of::<[IdtEntry; IDT_LEN]>() - 1) as u16;
        let base = core::ptr::addr_of!(IDT) as u64;
        let ptr = IdtPointer { limit, base };
        asm!(
            "lidt [{0}]",
            in(reg) &ptr,
            options(readonly, nostack, preserves_flags)
        );

        remap_pic();
        init_pit();
        outb(0x21, 0xFE);
        outb(0xA1, 0xFF);
        asm!("sti", options(nomem, nostack));
    }
}

/// Has the periodic timer actually fired?
///
/// `hlt` waits for an interrupt, so a wait that halts needs one that will arrive.
/// `IF` being set is not that: it says interrupts are *allowed*, not that a source
/// exists. Nor is "the PIT was configured" -- the first version of this function
/// reported exactly that, and it was wrong in the direction that matters: it said
/// yes on a path where the CPU then halted for good.
///
/// So this reports an observation instead. `TICK_COUNT` is incremented by the timer
/// handler and by nothing else, so a non-zero count is proof that an interrupt
/// arrived, which is the only thing a caller about to `hlt` needs to know.
///
/// Measured: on the Limine boot path the receive loop entered its first turn, read
/// the clock as having advanced 6.48 million cycles out of a budget of twelve
/// thousand million, halted, and never reached a second turn.
#[must_use]
pub fn timer_has_fired() -> bool {
    TICK_COUNT.load(Ordering::Relaxed) > 0
}

/// The network card's line.
///
/// Does as little as an interrupt handler should: asks the card whether the
/// interrupt was its own, lets it record what happened, and acknowledges the
/// controller. Reading the ring and deciding what a frame means happens in
/// ordinary code, where it can take as long as it needs.
extern "x86-interrupt" fn nic_isr(_frame: InterruptStackFrame) {
    let _ours = crate::drivers::e1000::on_interrupt();
    // SAFETY: port I/O to the two interrupt controllers, on the CPU that took
    // the interrupt. IRQ 11 arrives through the slave, so both halves are told
    // it is finished -- the slave first. Acknowledging only the master would
    // leave the slave holding the line and no further interrupt would arrive
    // from it, which is a quiet way to lose the timer as well.
    unsafe {
        outb(0xA0, 0x20);
        outb(0x20, 0x20);
    }
}

/// Let IRQ 11 through: its own bit on the slave, and the cascade on the master.
///
/// # Safety
///
/// Only after [`init`] has remapped the controllers and a handler is installed
/// for [`NIC_VECTOR`]. Unmasking a line with no handler behind it takes the
/// machine to an unhandled vector on the first interrupt.
pub unsafe fn unmask_nic_irq() {
    // SAFETY: the caller's contract. The masks are read back rather than
    // assumed, so this does not undo whatever else is masked.
    unsafe {
        let slave = inb(0xA1) & !(1 << 3);
        outb(0xA1, slave);
        let master = inb(0x21) & !(1 << 2);
        outb(0x21, master);
    }
}

extern "x86-interrupt" fn page_fault_isr(
    _frame: InterruptStackFrame,
    _error_code: u64,
) {
    // v0.5: page faults are real events the kernel already handles; counting them
    // costs one atomic and gives the translator a signal that is not hand-written.
    #[cfg(feature = "v05_translator")]
    crate::behavior::note_irq_page_fault();

    crate::mm::paging::handle_pf();
    // After handle_pf re-installs a placeholder mapping, returning here lets
    // the offending instruction retry and the kernel continues normally.
}

extern "x86-interrupt" fn syscall_isr(_frame: InterruptStackFrame) {
    // v0.5: stays at zero until something in ring 3 actually issues a syscall; wired
    // here so the count is real the moment that happens.
    #[cfg(feature = "v05_translator")]
    crate::behavior::note_irq_syscall();

    // Capture trap-time scratch registers before any Rust code clobbers
    // them. `lateout` with an empty asm tells the compiler that AFTER the
    // (empty) asm runs, the named register holds the named binding —
    // the compiler must therefore not destroy it pre-asm. With an empty
    // asm body, this captures the register state at function entry.
    let rax: u64;
    let rdi: u64;
    let rsi: u64;
    let rdx: u64;
    unsafe {
        asm!(
            "",
            lateout("rax") rax,
            lateout("rdi") rdi,
            lateout("rsi") rsi,
            lateout("rdx") rdx,
            options(nomem, nostack, preserves_flags),
        );
    }

    let result = crate::syscall::dispatch(crate::syscall::SyscallArgs {
        num: rax,
        a0: rdi,
        a1: rsi,
        a2: rdx,
    });

    // Place the syscall result in RAX so the user observes it after iretq.
    unsafe {
        asm!(
            "",
            in("rax") result,
            options(nomem, nostack, preserves_flags),
        );
    }
}

extern "x86-interrupt" fn timer_isr(_frame: InterruptStackFrame) {
    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    let _ = SCHED_INDEX.fetch_add(1, Ordering::Relaxed);

    // v0.5: one timer interrupt, observed.
    #[cfg(feature = "v05_translator")]
    crate::behavior::note_irq_timer();

    let mut best_idx: usize = 0;
    let mut best_ain: f64 = -1.0;
    let mut best_decision = Decision::Block;
    let mut best_ain_pct: u8 = 0;
    for (idx, task) in SCHED_TASKS.iter().enumerate() {
        // v0.5: the decision input comes from what this task's scheduling actually looked
        // like over the last window. Before the first window closes, and for any task with
        // nothing observed yet, this falls back to the task's old constant rather than
        // inventing a value.
        #[cfg(feature = "v05_translator")]
        let task_input = crate::behavior::input_for(idx, task.bias);
        #[cfg(not(feature = "v05_translator"))]
        let task_input = task.bias;

        let input = ComputeInput {
            bias: task_input,
            dimension: 9,
            samples: 64,
            seed: tick.wrapping_add(task.seed_offset),
        };
        let output = policy_compute(input);
        let decision = evaluate(POLICY, &output);
        if output.ain > best_ain {
            best_ain = output.ain;
            best_idx = idx;
            best_decision = decision;
            best_ain_pct = scale_ain_to_pct(output.ain);
        }
    }

    // v0.5: record which task won this tick, then close the window on its boundary.
    // This is the one signal that genuinely distinguishes the tasks today — it is produced
    // by the scheduler itself, not written into the table by hand.
    #[cfg(feature = "v05_translator")]
    {
        crate::behavior::note_scheduled(best_idx);
        crate::behavior::on_tick(tick, SCHED_TASKS.len() as u32);
    }

    let chosen = SCHED_TASKS[best_idx];

    // One call, panel and serial line together. They used to be two -- a direct call
    // here and a private serial loop below it -- and that is exactly how they came to
    // disagree on screen. The bytes on the wire are unchanged.
    crate::sched::sched_marker::record(
        crate::ui::gate_panel::Site::Scheduler,
        crate::ui::gate_panel::who_for_task(chosen.id),
        core::slice::from_ref(&chosen.id),
        best_ain_pct,
        best_decision,
    );

    unsafe {
        outb(0x20, 0x20);
    }
}

fn scale_ain_to_pct(ain: f64) -> u8 {
    let scaled = ain * 100.0 + 0.5;
    if scaled < 0.0 {
        0
    } else if scaled > 100.0 {
        100
    } else {
        scaled as u8
    }
}

unsafe fn remap_pic() {
    outb(0x20, 0x11);
    outb(0xA0, 0x11);
    outb(0x21, 0x20);
    outb(0xA1, 0x28);
    outb(0x21, 0x04);
    outb(0xA1, 0x02);
    outb(0x21, 0x01);
    outb(0xA1, 0x01);
    outb(0x21, 0xFF);
    outb(0xA1, 0xFF);
}

unsafe fn init_pit() {
    let divisor: u16 = 11932;
    outb(0x43, 0x36);
    outb(0x40, (divisor & 0xFF) as u8);
    outb(0x40, ((divisor >> 8) & 0xFF) as u8);
}
