//! The diagnostic's own small view of the CPU: COM1, a clock, a page-table walk, and an
//! exception catcher.
//!
//! None of this shares state with the kernel proper. The diagnostic image never enters
//! the kernel, and keeping its plumbing separate means a fault here cannot be blamed on
//! something the kernel set up, or the other way round.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::port::{inb, outb};

// ---------------------------------------------------------------------------
// COM1
// ---------------------------------------------------------------------------

const COM1: u16 = 0x3F8;

/// No UART answered at COM1 (the line status register read back 0xFF), so every write
/// is skipped instead of each one spinning out its timeout.
static COM1_ABSENT: AtomicBool = AtomicBool::new(false);

/// 115200 baud, 8N1, FIFO on, interrupts off.
pub fn com1_init() {
    // SAFETY: COM1's own registers, written in the standard order. Nothing else in the
    // diagnostic image drives this UART.
    unsafe {
        outb(COM1 + 1, 0x00); // no interrupts
        outb(COM1 + 3, 0x80); // DLAB on
        outb(COM1, 0x01); // divisor 1 = 115200
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03); // 8N1, DLAB off
        outb(COM1 + 2, 0xC7); // FIFO on, cleared, 14-byte threshold
        outb(COM1 + 4, 0x03); // DTR + RTS, loopback off
        if inb(COM1 + 5) == 0xFF {
            COM1_ABSENT.store(true, Ordering::Relaxed);
        }
    }
}

/// Write bytes to COM1. Waits a bounded time for room in the transmitter, then writes
/// anyway: a machine with no cable must not be slowed down by a serial port nobody is
/// reading.
pub fn com1_write(bytes: &[u8]) {
    if COM1_ABSENT.load(Ordering::Relaxed) {
        return;
    }
    for &b in bytes {
        for _ in 0..20_000 {
            // SAFETY: COM1 line status register; reading it has no side effect.
            if unsafe { inb(COM1 + 5) } & 0x20 != 0 {
                break;
            }
        }
        // SAFETY: COM1 transmit register.
        unsafe { outb(COM1, b) };
    }
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

#[inline]
#[must_use]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC reads a counter and changes nothing.
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    (u64::from(hi) << 32) | u64::from(lo)
}

/// TSC ticks per millisecond. A guess (2 GHz) until [`calibrate`] measures it.
static TSC_PER_MS: AtomicU64 = AtomicU64::new(2_000_000);
/// TSC value when the diagnostic started; uptime is measured from here.
static TSC_START: AtomicU64 = AtomicU64::new(0);

pub fn clock_start() {
    TSC_START.store(rdtsc(), Ordering::Relaxed);
}

#[must_use]
pub fn tsc_per_ms() -> u64 {
    TSC_PER_MS.load(Ordering::Relaxed)
}

/// Milliseconds since [`clock_start`].
#[must_use]
pub fn uptime_ms() -> u64 {
    rdtsc().wrapping_sub(TSC_START.load(Ordering::Relaxed)) / tsc_per_ms().max(1)
}

pub fn delay_ms(ms: u64) {
    let until = rdtsc().wrapping_add(ms.saturating_mul(tsc_per_ms()));
    while rdtsc() < until {
        core::hint::spin_loop();
    }
}

/// Read a CMOS register.
///
/// Bit 7 of the index port masks NMI. It is set on every access and so stays set: the
/// diagnostic has no NMI handler worth the name, and a stray NMI would stop the screen
/// with an "exception" that is not the machine's fault.
fn cmos_read(reg: u8) -> u8 {
    // SAFETY: the CMOS index and data ports. Selecting a register and reading it changes
    // nothing in the RTC.
    unsafe {
        outb(0x70, 0x80 | reg);
        inb(0x71)
    }
}

/// Wall-clock time from the CMOS RTC, `(hours, minutes, seconds)`.
#[must_use]
pub fn rtc_time() -> (u8, u8, u8) {
    let binary = cmos_read(0x0B) & 0x04 != 0;
    let conv = |v: u8| if binary { v } else { super::decode::bcd_to_bin(v) };
    let s = conv(cmos_read(0x00));
    let m = conv(cmos_read(0x02));
    // Bit 7 of the hour is the PM flag in 12-hour mode; drop it rather than print 140.
    let h = conv(cmos_read(0x04) & 0x7F);
    (h, m, s)
}

/// Wait for the RTC seconds register to change; `None` after about `budget` TSC ticks.
fn wait_rtc_edge(budget: u64) -> Option<u64> {
    let start = rdtsc();
    let first = cmos_read(0x00);
    loop {
        let now = rdtsc();
        if cmos_read(0x00) != first {
            return Some(now);
        }
        if now.wrapping_sub(start) > budget {
            return None;
        }
    }
}

/// Measure the TSC against one RTC second. Returns MHz, or `None` when the RTC did not
/// tick (the 2 GHz guess then stays).
///
/// Takes up to two seconds: one to find an edge, one to time the next.
pub fn calibrate() -> Option<u64> {
    // About eight seconds at 1 GHz, two at 4 GHz: enough for any edge to arrive.
    const BUDGET: u64 = 8_000_000_000;
    let a = wait_rtc_edge(BUDGET)?;
    let b = wait_rtc_edge(BUDGET)?;
    let per_ms = b.wrapping_sub(a) / 1000;
    // A second that measured as under 100 MHz or over 10 GHz is not a second.
    if !(100_000..=10_000_000).contains(&per_ms) {
        return None;
    }
    TSC_PER_MS.store(per_ms, Ordering::Relaxed);
    Some(per_ms / 1000)
}

// ---------------------------------------------------------------------------
// Control registers and the page-table walk
// ---------------------------------------------------------------------------

#[must_use]
pub fn read_cr2() -> u64 {
    let v: u64;
    // SAFETY: reading a control register has no side effect.
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[must_use]
pub fn read_cr3() -> u64 {
    let v: u64;
    // SAFETY: as above.
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[must_use]
pub fn read_cr4() -> u64 {
    let v: u64;
    // SAFETY: as above.
    unsafe { asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
static HHDM: AtomicU64 = AtomicU64::new(0);
static HHDM_SET: AtomicBool = AtomicBool::new(false);

pub fn set_hhdm(offset: u64) {
    HHDM.store(offset, Ordering::Relaxed);
    HHDM_SET.store(true, Ordering::Release);
}

#[must_use]
pub fn hhdm() -> Option<u64> {
    HHDM_SET.load(Ordering::Acquire).then(|| HHDM.load(Ordering::Relaxed))
}

/// Is this virtual address mapped right now? Answered by walking the live page tables.
///
/// This is what lets the diagnostic read ACPI tables and controller registers without
/// betting on which regions the bootloader chose to map: Limine's newer base revisions
/// leave reserved memory and MMIO out of the direct map, and a read there would fault.
/// Asking the page tables first turns that fault into a line saying "not mapped".
///
/// The tables themselves are read through the direct map, where Limine keeps them.
#[must_use]
pub fn va_mapped(va: u64) -> bool {
    let Some(off) = hhdm() else { return false };
    let levels: u32 = if read_cr4() & (1 << 12) != 0 { 5 } else { 4 };
    let mut table = read_cr3() & ADDR_MASK;
    for level in (0..levels).rev() {
        let shift = 12 + 9 * level;
        let index = (va >> shift) & 0x1FF;
        let slot = off.wrapping_add(table).wrapping_add(index * 8) as *const u64;
        // SAFETY: page-table pages are in bootloader-reclaimable memory, which Limine
        // maps in the direct map in every base revision.
        let entry = unsafe { core::ptr::read_volatile(slot) };
        if entry & 1 == 0 {
            return false;
        }
        // Bit 7 at the 1 GiB or 2 MiB level ends the walk with a large page.
        if (level == 1 || level == 2) && entry & (1 << 7) != 0 {
            return true;
        }
        table = entry & ADDR_MASK;
    }
    true
}

/// Every page of `[va, va + len)` is mapped.
#[must_use]
pub fn range_mapped(va: u64, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    let mut page = va & !0xFFF;
    let last = va.saturating_add(len - 1) & !0xFFF;
    loop {
        if !va_mapped(page) {
            return false;
        }
        if page >= last {
            return true;
        }
        page += 0x1000;
    }
}

/// The direct-map address of a physical one, if it can be read right now.
#[must_use]
pub fn phys_readable(pa: u64, len: u64) -> Option<u64> {
    let va = hhdm()?.checked_add(pa)?;
    range_mapped(va, len).then_some(va)
}

// ---------------------------------------------------------------------------
// Exception catcher
// ---------------------------------------------------------------------------

/// What the stubs leave on the stack, lowest address first.
#[repr(C)]
pub struct ExceptionFrame {
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

// One stub per exception vector, each exactly 16 bytes apart, so the IDT can be filled
// from a base address and a stride with no table of pointers (and so no relocations).
// Vectors where the CPU pushes no error code get a zero pushed in its place, so every
// frame has the same shape. The `.if` below is the architectural list of vectors that do
// push one; `decode::exception_has_error_code` is the same list, and its test checks it.
global_asm!(
    ".pushsection .text.zpl_diag_isr, \"ax\"",
    ".balign 16",
    ".global zpl_diag_isr_base",
    "zpl_diag_isr_base:",
    ".altmacro",
    ".macro ZPL_DIAG_ISR v",
    "  .balign 16",
    "  .if (\\v == 8) || ((\\v >= 10) && (\\v <= 14)) || (\\v == 17) || (\\v == 21) || (\\v == 29) || (\\v == 30)",
    "  .else",
    "    push 0",
    "  .endif",
    "  push \\v",
    "  jmp zpl_diag_isr_common",
    ".endm",
    ".set zpl_v, 0",
    ".rept 32",
    "  ZPL_DIAG_ISR %zpl_v",
    "  .set zpl_v, zpl_v + 1",
    ".endr",
    ".noaltmacro",
    ".balign 16",
    "zpl_diag_isr_common:",
    "  mov rdi, rsp",
    "  and rsp, -16",
    "  call {handler}",
    "  ud2",
    ".popsection",
    handler = sym exception_entry,
);

extern "C" {
    static zpl_diag_isr_base: u8;
}

const ISR_STRIDE: u64 = 16;

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

const IDT_EMPTY: IdtEntry = IdtEntry {
    offset_low: 0,
    selector: 0,
    ist: 0,
    type_attr: 0,
    offset_mid: 0,
    offset_high: 0,
    zero: 0,
};

static mut IDT: [IdtEntry; 32] = [IDT_EMPTY; 32];

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

/// Point the CPU at an IDT that catches all 32 exceptions.
///
/// Limine leaves the IDT undefined. Without this, the first fault triple-faults and the
/// machine resets -- and a machine that resets goes straight back to the firmware, which
/// boots the installed system. That is exactly what one of the target machines does, so
/// a fault has to become a line on the screen instead.
pub fn install_idt() {
    let cs: u16;
    // SAFETY: reading CS has no side effect.
    unsafe { asm!("mov {:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags)) };
    // Only the address of the symbol is taken, never its contents.
    let base = core::ptr::addr_of!(zpl_diag_isr_base) as u64;
    // SAFETY: single-threaded, interrupts are off, and nothing else touches the table.
    let idt = unsafe { &mut *core::ptr::addr_of_mut!(IDT) };
    for (v, entry) in idt.iter_mut().enumerate() {
        let addr = base + v as u64 * ISR_STRIDE;
        *entry = IdtEntry {
            offset_low: (addr & 0xFFFF) as u16,
            selector: cs,
            ist: 0,
            type_attr: 0x8E, // present, ring 0, 64-bit interrupt gate
            offset_mid: ((addr >> 16) & 0xFFFF) as u16,
            offset_high: (addr >> 32) as u32,
            zero: 0,
        };
    }
    let ptr = IdtPointer {
        limit: (core::mem::size_of::<[IdtEntry; 32]>() - 1) as u16,
        base: idt.as_ptr() as u64,
    };
    // SAFETY: `ptr` describes a table that lives for the rest of the boot.
    unsafe { asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags)) };
}

extern "C" fn exception_entry(frame: *const ExceptionFrame) -> ! {
    // SAFETY: the stub passes its own stack pointer, which points at a full frame.
    let frame = unsafe { &*frame };
    super::report_exception(frame);
    halt_forever()
}

pub fn halt_forever() -> ! {
    loop {
        // SAFETY: stops the CPU until the next interrupt, and interrupts are off.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
