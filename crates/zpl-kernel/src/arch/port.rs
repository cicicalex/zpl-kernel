//! The one door to the x86 I/O ports.
//!
//! **Why this exists.** The same two instructions, `in` and `out`, used to be written
//! out in eight different files, each with its own `unsafe` block and its own idea of
//! what the safety argument was. Eight copies of the same three lines is eight places
//! to get the operand constraints wrong, and no place to write down, once, what a
//! caller has to guarantee.
//!
//! # Safety, stated once
//!
//! Every function here is `unsafe`, and the obligation is always the same three things:
//!
//! 1. **The port number is a real port on this machine**, and the width matches what
//!    the device expects. A 16-bit read from an 8-bit register returns whatever the
//!    bus leaves on the upper half.
//! 2. **No one else is driving the same port.** Port I/O has no lock and no ownership;
//!    two drivers sharing a port interleave their reads and writes silently. In this
//!    kernel that is kept true by having one module per device.
//! 3. **The side effect is intended.** Many ports are not memory: reading one can
//!    acknowledge an interrupt, advance a FIFO, or start a transfer. `inb` is not a
//!    peek.
//!
//! A caller that holds all three does not need to repeat them; a caller that does
//! something unusual -- masking interrupts around a sequence, or touching a port during
//! early boot before a driver owns it -- says *that*, which is the part worth saying.
//!
//! # Why there is no `outw`
//!
//! Nothing in this kernel sends 16-bit words to a port, and one thing deliberately must
//! not: the ATA driver is read-only by construction, and the absence of a 16-bit write
//! is half of how that is enforced. Adding `outw` here would quietly remove that, so it
//! is left out until something needs it, and then the ATA module's own test will be the
//! one to argue with.

// Bare metal, plus host test builds -- the module's own test reads the kernel's source
// with `include_str!` and needs to be compiled somewhere `cargo test` runs. The `asm!`
// below compiles on an x86-64 host too; it simply never executes there.
#![cfg(any(all(target_os = "none", target_arch = "x86_64"), test))]

use core::arch::asm;

/// Read one byte from an I/O port.
///
/// # Safety
/// See the module header.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

/// Read one 16-bit word from an I/O port.
///
/// # Safety
/// See the module header.
#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        asm!(
            "in ax, dx",
            out("ax") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

/// Read one 32-bit word from an I/O port.
///
/// # Safety
/// See the module header.
#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!(
            "in eax, dx",
            out("eax") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

/// Write one byte to an I/O port.
///
/// # Safety
/// See the module header, and note obligation 3 in particular: a byte written to a port
/// is a command to a device, not a store to memory.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Write one 32-bit word to an I/O port.
///
/// # Safety
/// See the module header.
#[inline]
pub unsafe fn outl(port: u16, value: u32) {
    unsafe {
        asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

#[cfg(test)]
mod tests {
    /// Nobody writes `in` or `out` outside this module.
    ///
    /// The point of gathering port I/O here is lost the moment a ninth copy appears, and
    /// a ninth copy appears the way the first eight did: somebody needs one byte on the
    /// serial port and writes three lines of assembly rather than finding the door. So
    /// the rule is checked rather than asked for.
    ///
    /// The one exception is named, with its reason, below. A list of exceptions that
    /// each say why is a different thing from a rule nobody enforces.
    #[test]
    fn port_io_lives_only_here() {
        // Files are listed rather than walked: `include_str!` needs a literal path, and
        // a literal list is also a thing a reviewer can read. A new file with port I/O
        // in it is caught by `arch-metrics`, which does walk the tree.
        let files: &[(&str, &str)] = &[
            ("boot.rs", include_str!("../boot.rs")),
            ("interrupts.rs", include_str!("../interrupts.rs")),
            ("drivers/kbd.rs", include_str!("../drivers/kbd.rs")),
            ("panic.rs", include_str!("../panic.rs")),
            ("drivers/serial.rs", include_str!("../drivers/serial.rs")),
            ("qemu_exit.rs", include_str!("../qemu_exit.rs")),
            ("agent_debug_com1.rs", include_str!("../agent_debug_com1.rs")),
            ("drivers/pci.rs", include_str!("../drivers/pci.rs")),
            ("drivers/e1000.rs", include_str!("../drivers/e1000.rs")),
            ("drivers/virtio_net.rs", include_str!("../drivers/virtio_net.rs")),
        ];
        for (name, src) in files {
            for (i, line) in src.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for insn in ["in al, dx", "out dx, al", "in ax, dx", "out dx, ax",
                             "in eax, dx", "out dx, eax"] {
                    assert!(
                        !code.contains(insn),
                        "{name}:{} writes `{insn}` itself; port I/O goes through \
                         `arch::port`, where the safety argument is written once",
                        i + 1
                    );
                }
            }
        }
    }
}
