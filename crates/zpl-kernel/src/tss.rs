//! Task State Segment (TSS) and `ltr` plumbing for ring-3 support.
//!
//! Layout summary (set up by `start.rs` global asm + this module):
//!
//! - GDT[0x00]: null
//! - GDT[0x08]: kernel code (DPL=0)
//! - GDT[0x10]: kernel data (DPL=0)
//! - GDT[0x18]: user code (DPL=3)
//! - GDT[0x20]: user data (DPL=3)
//! - GDT[0x28..0x38]: 16-byte 64-bit TSS descriptor — patched by `init`.
//!
//! On `init`, this module:
//! 1. Records the kernel ring-0 stack pointer in `Tss::rsp0` so that any
//!    transition from a less-privileged ring back into ring 0 lands on a
//!    known kernel stack.
//! 2. Builds the 16-byte TSS descriptor pointing at the static `TSS`
//!    structure and writes it into `zpl_gdt[5..7]`.
//! 3. Loads the task register with `ltr 0x28`.
//!
//! Per Intel SDM volume 3, the in-memory `TSS` does not need to be 16-byte
//! aligned for correctness, but we align it for cache friendliness.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

#[repr(C, align(16))]
struct TssAligned {
    inner: Tss,
}

const TSS_LIMIT_BYTES: u64 = (core::mem::size_of::<Tss>() - 1) as u64;

static mut TSS: TssAligned = TssAligned {
    inner: Tss {
        reserved0: 0,
        rsp0: 0,
        rsp1: 0,
        rsp2: 0,
        reserved1: 0,
        ist1: 0,
        ist2: 0,
        ist3: 0,
        ist4: 0,
        ist5: 0,
        ist6: 0,
        ist7: 0,
        reserved2: 0,
        reserved3: 0,
        iomap_base: core::mem::size_of::<Tss>() as u16,
    },
};

extern "C" {
    static mut zpl_gdt: [u64; 7];
}

/// Point GDTR at our static `zpl_gdt` and reload CS/ds/… Limine leaves its own
/// GDT active; without this, `tss::init` patches memory the CPU never consults
/// and the first IRQ after `sti` faults on selector `0x28`.
#[cfg(not(feature = "qemu_boot"))]
unsafe fn reload_zpl_gdt_active() {
    #[repr(C, packed)]
    struct Gdtr {
        limit: u16,
        base: u64,
    }
    let base = addr_of_mut!(zpl_gdt) as u64;
    let gdtr = Gdtr {
        limit: (core::mem::size_of::<[u64; 7]>() - 1) as u16,
        base,
    };
    core::arch::asm!(
        "lgdt [{gdtr}]",
        "lea rax, [rip + 2f]",
        "push 0x08",
        "push rax",
        "retfq",
        "2:",
        gdtr = in(reg) &gdtr as *const Gdtr as u64,
        out("rax") _,
        options(nostack)
    );
    core::arch::asm!(
        "mov ax, 0x10",
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",
        "mov ss, ax",
        options(nostack, preserves_flags)
    );
}

/// Point `rsp0` at a different kernel stack.
///
/// Needed because `rsp0` is where the CPU puts the interrupt frame when a ring-3
/// program traps, and it is captured once at boot from whatever depth the boot
/// function was at. A syscall taken while the shell is running would therefore start
/// its own frames at that address and grow *down* -- straight through the shell's
/// live frames, which sit below it. Giving the user program its own kernel stack for
/// the length of its run keeps the two apart.
///
/// # Safety
/// `rsp0` must be the top of a stack nothing else is using, 16-byte aligned, and
/// [`init`] must already have run. The caller must put the old value back once no
/// ring-3 code can trap any more.
pub unsafe fn set_rsp0(rsp0: u64) {
    unsafe {
        (*addr_of_mut!(TSS)).inner.rsp0 = rsp0;
    }
}

/// What `rsp0` is now, so a caller can put it back.
#[must_use]
pub fn rsp0() -> u64 {
    // SAFETY: a plain read of a field only this module writes, on the boot CPU.
    unsafe { (*addr_of!(TSS)).inner.rsp0 }
}

/// Configure the TSS descriptor in `zpl_gdt`, install `rsp0`, and load TR.
///
/// # Safety
/// Must run once on the bootstrap CPU after the GDT is loaded but before
/// any transition to ring 3.
pub unsafe fn init(rsp0: u64) {
    unsafe {
        #[cfg(not(feature = "qemu_boot"))]
        reload_zpl_gdt_active();

        let tss_ptr = addr_of_mut!(TSS) as u64;
        (*addr_of_mut!(TSS)).inner.rsp0 = rsp0;

        // 16-byte 64-bit TSS descriptor laid out per Intel SDM Vol 3
        // Figure 8-4. Type=0x9 (TSS available), DPL=0, P=1.
        let limit_lo = TSS_LIMIT_BYTES & 0xFFFF;
        let limit_hi = (TSS_LIMIT_BYTES >> 16) & 0x0F;
        let base_lo = tss_ptr & 0xFF_FFFF;
        let base_mid = (tss_ptr >> 24) & 0xFF;
        let base_hi = (tss_ptr >> 32) & 0xFFFF_FFFF;

        let access_byte: u64 = 0x89; // P=1 DPL=0 type=64-bit available TSS
        let flags_byte: u64 = 0x00; // G=0, AVL=0

        let low = limit_lo
            | (base_lo << 16)
            | (access_byte << 40)
            | ((flags_byte | limit_hi) << 52)
            | (base_mid << 56);
        let high = base_hi;

        let gdt = addr_of_mut!(zpl_gdt) as *mut u64;
        core::ptr::write_volatile(gdt.add(5), low);
        core::ptr::write_volatile(gdt.add(6), high);

        asm!(
            "ltr {0:x}",
            in(reg) 0x28u16,
            options(nostack, nomem, preserves_flags)
        );
    }
}
