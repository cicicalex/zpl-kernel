//! QEMU `isa-debug-exit` device wiring.
//!
//! When QEMU is launched with
//! `-device isa-debug-exit,iobase=0xf4,iosize=0x04`,
//! writing a 32-bit value `v` to port `0xF4` causes QEMU to exit with
//! process exit code `(v << 1) | 1`. Common conventions:
//!
//! - `qemu_exit_success()` → process exit code `1`
//! - `qemu_exit_failure(n)` → process exit code `(n << 1) | 1`
//!
//! Used by the kernel panic handler and integration test
//! harnesses to propagate pass/fail from the guest to the host.

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const PORT: u16 = 0xF4;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline(always)]
unsafe fn outl(port: u16, value: u32) {
    // SAFETY: the debug-exit port, written once, deliberately, to end the machine.
    unsafe { crate::arch::port::outl(port, value) }
}

/// Exit QEMU cleanly with code `(0 << 1) | 1 == 1`.
pub fn qemu_exit_success() -> ! {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    unsafe {
        outl(PORT, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Exit QEMU cleanly with code `(n << 1) | 1`.
pub fn qemu_exit_failure(n: u32) -> ! {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    unsafe {
        outl(PORT, n);
    }
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    {
        let _ = n;
    }
    loop {
        core::hint::spin_loop();
    }
}
