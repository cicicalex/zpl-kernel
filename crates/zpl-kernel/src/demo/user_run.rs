//! Enter a ring-3 program and come back when it exits.
//!
//! The existing ring-3 paths (`ring3_demo`, `elf_demo`) never return: the program
//! calls `zpl_exit` and the kernel stops the machine. That is fine for a boot that
//! ends there, and useless for a prompt -- `run <file>` has to hand control back.
//!
//! This is not a context switch and does not pretend to be one. There is no second
//! address space, no saved register file, no scheduler. It is a one-shot
//! `setjmp`/`longjmp` across the ring boundary: the kernel stack pointer is written
//! down before `iretq`, and `zpl_exit` puts it back and returns. One program at a
//! time, no nesting, and if the program never calls `zpl_exit` the kernel never comes
//! back -- the same as today.
//!
//! **Two stacks, and that is the part that matters.** `TSS.rsp0` is where the CPU
//! puts the interrupt frame when ring 3 traps, and it is captured once at boot. A
//! syscall taken while the shell is running would start its frames there and grow
//! down, through the shell's own live frames, which sit below it. So the program gets
//! a kernel stack of its own for the length of its run, and `rsp0` is put back
//! afterwards.

// Gated on the ELF loader alone, not on the shell: the mechanism is what needs
// proving, and the `-kernel` build has the loader but cannot have the shell, which
// needs the framebuffer console. `user_run_demo` exercises it there.
#![cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "elf_demo", feature = "user_programs")
))]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::mm::paging;

const USER_CODE_SELECTOR_DPL3: u64 = 0x18 | 3;
const USER_DATA_SELECTOR_DPL3: u64 = 0x20 | 3;
/// IF set, everything else clear. The program runs with interrupts on, like the rest
/// of the kernel: a program stuck in a loop must not also freeze the timer.
const USER_RFLAGS: u64 = 0x202;

/// Where the program's own user stack goes. Far enough above the load address that a
/// program growing its stack downward does not reach its own code.
const USER_STACK_VIRT: u64 = paging::USER_REGION_BASE + 0x30_000;
const USER_STACK_TOP: u64 = USER_STACK_VIRT + 0x1000;

/// The kernel stack a trapping user program lands on. 16 KiB, which is four times the
/// deepest syscall path measured here and leaves room for the fault handlers.
const SYSCALL_STACK_BYTES: usize = 16 * 1024;

#[repr(C, align(16))]
struct SyscallStack([u8; SYSCALL_STACK_BYTES]);

static mut SYSCALL_STACK: SyscallStack = SyscallStack([0; SYSCALL_STACK_BYTES]);

/// The kernel `rsp` to come back to, written just before `iretq`.
static RESUME_RSP: AtomicU64 = AtomicU64::new(0);
/// Whether a shell-launched program is running right now. `zpl_exit` reads this to
/// decide between coming back here and stopping the machine, and it is what makes
/// nesting impossible: a second `enter` while one is live is refused.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// `rsp0` as it was before the program started.
static SAVED_RSP0: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    /// Another program is already running. There is one stack, so there is one slot.
    AlreadyRunning,
    /// The user stack could not be mapped.
    NoStack,
}

/// Is a shell-launched program on the CPU right now?
#[must_use]
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// Map the user stack for a program that is about to run.
///
/// # Safety
/// `paging::init` must have run.
unsafe fn map_user_stack() -> Result<(), RunError> {
    let phys = crate::mm::frame_alloc::alloc_frame().ok_or(RunError::NoStack)?;
    // SAFETY: the caller's contract, and `phys` came from the frame allocator, so it
    // is a frame nothing else holds.
    unsafe {
        paging::map_4k_user_page(USER_STACK_VIRT, phys).map_err(|_| RunError::NoStack)?;
    }
    Ok(())
}

/// Enter `entry` in ring 3 and return the exit code the program passed to `zpl_exit`.
///
/// # Safety
/// `entry` must be the entry point of an image already loaded and mapped as user
/// pages, `tss::init` must have run, and no other ring-3 program may be live.
pub unsafe fn enter(entry: u64) -> Result<u64, RunError> {
    if RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(RunError::AlreadyRunning);
    }
    // SAFETY: the caller's contract.
    if let Err(e) = unsafe { map_user_stack() } {
        RUNNING.store(false, Ordering::Release);
        return Err(e);
    }

    // The program's own kernel stack, and a note of what `rsp0` was.
    let stack_top = {
        let base = &raw const SYSCALL_STACK as u64;
        // Grows down from the end, 16-byte aligned.
        (base + SYSCALL_STACK_BYTES as u64) & !0xF
    };
    SAVED_RSP0.store(crate::tss::rsp0(), Ordering::Release);
    // SAFETY: `stack_top` is the top of a static nothing else uses, and it is put
    // back below as soon as the program can no longer trap.
    unsafe { crate::tss::set_rsp0(stack_top) };

    let code = unsafe { enter_and_return(entry) };

    // SAFETY: no ring-3 code can trap any more -- `RUNNING` is false by the time the
    // caller sees this, and the resume path below is the only way here.
    unsafe { crate::tss::set_rsp0(SAVED_RSP0.load(Ordering::Acquire)) };
    RUNNING.store(false, Ordering::Release);
    Ok(code)
}

/// The ring transition itself.
///
/// Saves the callee-saved registers and the resume address on the current kernel
/// stack, writes `rsp` down where [`resume`] can find it, and `iretq`s into the
/// program. [`resume`] brings control back to the label, where the registers are
/// restored and the exit code is in `rax`.
///
/// # Safety
/// See [`enter`]; this is its inner half and must not be called directly.
#[inline(never)]
unsafe fn enter_and_return(entry: u64) -> u64 {
    let code: u64;
    // SAFETY: the frame this builds is exactly the one `iretq` consumes, and the only
    // path back is `resume`, which restores `rsp` to the value stored here. The
    // callee-saved registers are pushed by hand because `clobber_abi` does not cover
    // them and the jump back does not go through a normal return.
    unsafe {
        core::arch::asm!(
            "push rbp",
            "push rbx",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // Where `resume` will return to. `rax` is already declared as the
            // output, so it is a register the compiler knows about and it can
            // serve as scratch here -- a generic `out(reg)` cannot, because
            // `clobber_abi` requires every output to name its register.
            "lea rax, [rip + 22f]",
            "push rax",
            "mov [{slot}], rsp",
            // The `iretq` frame, in the order the CPU pops it.
            "push {ss}",
            "push {ustack}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            // `resume` lands here with the exit code in rax.
            "22:",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",
            slot = in(reg) RESUME_RSP.as_ptr(),
            ss = in(reg) USER_DATA_SELECTOR_DPL3,
            ustack = in(reg) USER_STACK_TOP,
            rflags = in(reg) USER_RFLAGS,
            cs = in(reg) USER_CODE_SELECTOR_DPL3,
            rip = in(reg) entry,
            out("rax") code,
            clobber_abi("sysv64"),
        );
    }
    code
}

/// Go back to the shell with `code`. Called from `zpl_exit`, and never returns.
///
/// # Safety
/// [`is_running`] must be true, i.e. [`enter`] is on the stack below this call.
pub unsafe fn resume(code: u64) -> ! {
    let rsp = RESUME_RSP.load(Ordering::Acquire);
    // SAFETY: `rsp` was written by `enter_and_return` on the kernel stack that is
    // still live below this frame -- the syscall ran on a different stack, so nothing
    // has overwritten it. The word at `rsp` is the resume address it pushed.
    unsafe {
        core::arch::asm!(
            "mov rsp, {rsp}",
            "ret",
            rsp = in(reg) rsp,
            in("rax") code,
            options(noreturn),
        );
    }
}
