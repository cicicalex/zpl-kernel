//! Ring 0 -> Ring 3 -> Ring 0 round-trip demo.
//!
//! Flow when the kernel is built with `--features ring3_demo`:
//!
//! 1. Allocate two physical frames, map them user-accessible (`U=1`)
//!    at well-known virtual addresses (user-code page + user-stack page).
//! 2. Copy a tiny ring-3 program into the user-code page: `int 0x80`
//!    followed by `jmp $-2` so the program loops forever after issuing
//!    its first syscall (the syscall ISR is what terminates QEMU).
//! 3. Build an `iretq` frame on the kernel stack with `cs|3`, `ss|3`,
//!    `rflags=0x202`, ring-3 RIP, ring-3 RSP and execute `iretq`.
//! 4. The CPU enters ring 3, runs `int 0x80`, traps back into the kernel
//!    via IDT[0x80] (DPL=3 trap gate). The ISR emits a marker on COM1
//!    and exits QEMU with `qemu_exit_success`.
//!
//! Standard x86_64 ring transition: Intel SDM volume 3, chapter 6.12.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::arch::asm;

use crate::mm::frame_alloc::alloc_frame;
use crate::mm::paging;

const USER_CODE_VIRT: u64 = paging::USER_REGION_BASE + 0x10_000;
const USER_DATA_VIRT: u64 = paging::USER_REGION_BASE + 0x10_800;
const USER_STACK_VIRT_BASE: u64 = paging::USER_REGION_BASE + 0x20_000;
const USER_STACK_TOP: u64 = USER_STACK_VIRT_BASE + 0x1000;

const KERNEL_CODE_SELECTOR: u64 = 0x08;
const USER_CODE_SELECTOR_DPL3: u64 = 0x18 | 3;
const USER_DATA_SELECTOR_DPL3: u64 = 0x20 | 3;
const USER_RFLAGS: u64 = 0x202; // IF=1 reserved=1

/// Encoded ring-3 program for syscall ABI v1 (see `docs/SYSCALL_ABI.md`):
/// ```text
///   mov eax, 0          ; SYS_LOG
///   mov edi, 0x40010800 ; ptr to "hi"
///   mov esi, 2          ; len
///   int 0x80
///   mov eax, 4          ; SYS_EXIT
///   mov edi, 0
///   int 0x80
/// ```
/// 29 bytes total: 27 instruction bytes + 2 trailing `int 0x80` bytes.
/// All fit inside the single 4 KiB user code page at `USER_CODE_VIRT`.
const RING3_PROGRAM: [u8; 29] = [
    0xB8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0
    0xBF, 0x00, 0x08, 0x01, 0x40, // mov edi, 0x40010800
    0xBE, 0x02, 0x00, 0x00, 0x00, // mov esi, 2
    0xCD, 0x80,                   // int 0x80 (SYS_LOG)
    0xB8, 0x04, 0x00, 0x00, 0x00, // mov eax, 4 (SYS_EXIT)
    0xBF, 0x00, 0x00, 0x00, 0x00, // mov edi, 0
    0xCD, 0x80,                   // int 0x80 (SYS_EXIT)
];

const RING3_DATA: [u8; 2] = [b'h', b'i'];

#[derive(Debug, Clone, Copy)]
pub enum Ring3DemoErr {
    NoCodeFrame,
    NoStackFrame,
    MapCodeFailed,
    MapStackFailed,
}

/// Prepare the user-mode code and stack pages but do not yet transition.
pub unsafe fn prepare() -> Result<(), Ring3DemoErr> {
    let code_phys = alloc_frame().ok_or(Ring3DemoErr::NoCodeFrame)?;
    let stack_phys = alloc_frame().ok_or(Ring3DemoErr::NoStackFrame)?;

    unsafe {
        paging::map_4k_user_page(USER_CODE_VIRT, code_phys)
            .map_err(|_| Ring3DemoErr::MapCodeFailed)?;
        paging::map_4k_user_page(USER_STACK_VIRT_BASE, stack_phys)
            .map_err(|_| Ring3DemoErr::MapStackFailed)?;
    }

    unsafe {
        let dst = USER_CODE_VIRT as *mut u8;
        for (i, byte) in RING3_PROGRAM.iter().enumerate() {
            core::ptr::write_volatile(dst.add(i), *byte);
        }
        let data_dst = USER_DATA_VIRT as *mut u8;
        for (i, byte) in RING3_DATA.iter().enumerate() {
            core::ptr::write_volatile(data_dst.add(i), *byte);
        }
    }
    Ok(())
}

/// Acknowledge the kernel binary slot so an external observer can keep
/// the boot path's `[ZPL-BOOT] halt loop entered` marker quiet (CPU is in
/// ring 3 doing user-mode work). Kept for symmetry with other demos.
pub fn ring3_marker(label: &[u8]) {
    use crate::drivers::console::boot_probe_byte;
    for byte in label.iter() {
        boot_probe_byte(*byte);
    }
}

/// Push an `iretq` frame onto the current ring-0 stack and execute it,
/// transferring control to ring 3.
///
/// This function never returns: the only way back into the kernel is via
/// the `int 0x80` ISR, which terminates QEMU itself.
///
/// # Safety
/// Caller must guarantee that `prepare()` succeeded, that `tss::init`
/// installed `rsp0`, and that the `int 0x80` IDT vector is configured
/// with DPL=3 so ring 3 can invoke it.
#[inline(never)]
pub unsafe fn enter_ring3() -> ! {
    let _kernel_cs = KERNEL_CODE_SELECTOR;
    let user_cs = USER_CODE_SELECTOR_DPL3;
    let user_ss = USER_DATA_SELECTOR_DPL3;
    let user_rip = USER_CODE_VIRT;
    let user_rsp = USER_STACK_TOP;
    let rflags = USER_RFLAGS;

    unsafe {
        asm!(
            // Build iretq frame: ss, rsp, rflags, cs, rip (in that order
            // pushed; iretq pops in reverse).
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss = in(reg) user_ss,
            rsp = in(reg) user_rsp,
            rflags = in(reg) rflags,
            cs = in(reg) user_cs,
            rip = in(reg) user_rip,
            options(noreturn),
        );
    }
}
