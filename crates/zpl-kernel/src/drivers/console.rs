//! Writing a line where a person can see it.
//!
//! **Why this is its own module.** These are the primitives every other part of the
//! kernel needs: put a byte on the serial port, mirror a marker to whatever console
//! this boot path has, hold off interrupts while a whole line goes out. They used to
//! live in `boot.rs`, which meant eight modules named `crate::boot::` in order to print
//! something -- and `boot` names them back, because it starts them. Eight of the twelve
//! mutually dependent pairs in this kernel were that, and only that.
//!
//! Moving them here breaks those edges without changing a byte of what they do: output
//! depends on nothing, and everything depends on output.

/// COM1-only marker emission (no VGA mirror). Used by `paging` while installing
/// the HHDM+0xB8000 leaf so we do not recurse into `emit_critical_marker`.
///
/// Its only caller, `paging::map_vga_text_buffer_hhdm`, is compiled out of a
/// `qemu_boot` build, so this is gated the same way rather than left to raise a
/// dead-code warning there. The host stub below carries the same note.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot")
))]
pub(crate) fn boot_probe_slice_no_vga(bytes: &[u8]) {
    with_irq_masked(|| {
        for &b in bytes {
            boot_probe_byte(b);
        }
    });
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
#[allow(dead_code)] // Used only from `paging` on the `target_os = "none"` Limine build.
pub(crate) fn boot_probe_slice_no_vga(_bytes: &[u8]) {}

/// Put one byte on COM1, and nothing else.
///
/// Split out of [`boot_probe_byte`] so the shell can echo what is typed without the
/// running summary seeing it. Everything the kernel says about itself goes through
/// `boot_probe_byte` and is watched; what a person types goes through here and is not.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) fn com1_put_byte(byte: u8) {
    // Wait for the transmitter to be empty, then hand over the byte. Two ports, both
    // the UART's, both owned by the kernel's own output path.
    // SAFETY: see `arch::port` -- COM1, single writer, and the side effect is the point.
    unsafe {
        while crate::arch::port::inb(0x3FD) & 0x20 == 0 {
            core::hint::spin_loop();
        }
        crate::arch::port::outb(0x3F8, byte);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) fn boot_probe_byte(byte: u8) {
    com1_put_byte(byte);
    // v0.4: every COM1 byte the kernel writes about itself passes here, so this is where
    // the running summary watches lines. It only folds in the ones the determinism method
    // keeps, which is what makes the on-screen hash reproducible from the serial log.
    #[cfg(feature = "v04_menu")]
    crate::ui::v04_status::feed_byte(byte);
}

/// Write shell output to COM1 and to whichever console is mirroring.
///
/// Deliberately **not** `emit_critical_marker`: that feeds every byte to the running
/// summary, so a half-typed line would splice into whatever marker the halt loop emitted
/// next and change the on-screen hash. Nothing a person types can reach the hash through
/// this path, which is a stronger guarantee than the v0.4 menu's (its lines are excluded
/// by name; these are never offered).
#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "shell"))]
pub(crate) fn shell_echo(bytes: &[u8]) {
    with_irq_masked(|| {
        for byte in bytes.iter() {
            com1_put_byte(*byte);
        }
        #[cfg(feature = "vga_crit_mirror")]
        {
            if crate::mm::paging::is_vga_hhdm_mapped() {
                crate::drivers::vga::set_color(crate::drivers::vga::COLOR_WHITE);
                for byte in bytes.iter() {
                    crate::drivers::vga::write_byte(*byte);
                }
            }
        }
        #[cfg(all(not(feature = "qemu_boot"), feature = "fb_crit_mirror"))]
        {
            if crate::drivers::fbcon::is_fb_ready() {
                crate::drivers::fbcon::set_color(crate::drivers::fbcon::FB_WHITE);
                crate::drivers::fbcon::write_bytes(bytes);
            }
        }
    });
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
pub(crate) fn boot_probe_byte(_byte: u8) {}

/// Run `f` with maskable interrupts disabled so timer-driven `[ZPL-SCHED …]`
/// lines cannot splice a single legacy `[ZPL-*] …` COM1 line.
///
/// Restores the previous **IF** (bit 9 of RFLAGS): unconditional `sti` after
/// `cli` would force interrupts on and can fault before `interrupts::init`.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) fn with_irq_masked<R>(f: impl FnOnce() -> R) -> R {
    const RFLAGS_IF: u64 = 1 << 9;
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, nostack));
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    let out = f();
    if (flags & RFLAGS_IF) != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
    out
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
pub(crate) fn with_irq_masked<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// A fixed-size buffer for markers that are assembled from pieces.
///
/// Early boot has no allocator, so a marker with a number in it used to be
/// written one byte at a time straight to COM1 — which meant it could never be
/// mirrored to the screen as one coloured line, and so never appeared there at
/// all. Building the line here first and handing it to `emit_critical_marker`
/// puts it on the same path as every other marker.
///
/// 192 bytes: the longest such line today is the receive report at about 140 --
/// length, scan count, cycles spent, cycles allowed, the sender's address and
/// the verdict. The PCI device line, the previous longest, is about 94, and the
/// SMP probe before that 52. It has grown twice; both times something that did
/// not fit was found by reading a log that had gone wrong, which is why `push`
/// now guarantees a truncated line still ends.
pub(crate) struct MarkerLine {
    buf: [u8; Self::CAPACITY],
    len: usize,
}

impl MarkerLine {
    /// How much one marker may be. Named rather than repeated, because the
    /// first time this grew, a test that had the old number written into it
    /// failed for the right reason and the wrong one: the buffer was fine and
    /// the literal was stale.
    pub(crate) const CAPACITY: usize = 192;

    pub(crate) const fn new() -> Self {
        Self { buf: [0u8; Self::CAPACITY], len: 0 }
    }

    /// Append bytes, dropping anything past the end rather than panicking:
    /// a truncated marker in early boot is better than a fault.
    ///
    /// A marker that does not fit still ends in a newline. That is not tidiness:
    /// a truncated line used to lose its terminator and run straight into the
    /// next marker, so two good lines became one corrupt one and the log said
    /// something neither of them said. Found exactly that way.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        for byte in bytes.iter() {
            if self.len < self.buf.len() {
                self.buf[self.len] = *byte;
                self.len += 1;
            } else {
                let last = self.buf.len() - 1;
                self.buf[last] = b'\n';
                return;
            }
        }
    }

    pub(crate) fn push_dec(&mut self, mut value: u64) {
        if value == 0 {
            self.push(b"0");
            return;
        }
        let mut digits = [0u8; 20];
        let mut idx = digits.len();
        while value > 0 {
            idx -= 1;
            digits[idx] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        self.push(&digits[idx..]);
    }

    /// Sixteen lower-case hex digits, zero-padded — the format the address
    /// markers have always printed.
    // Its one caller, `paging::emit_pdpt_va_diag`, is bare-metal only, so a host
    // build sees this as dead. Keeping the buffer itself host-visible is what lets
    // the unit tests below run at all.
    #[cfg_attr(
        not(all(target_os = "none", target_arch = "x86_64")),
        allow(dead_code)
    )]
    pub(crate) fn push_hex64(&mut self, value: u64) {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for n in (0..16).rev() {
            let idx = ((value >> (n * 4)) & 0xf) as usize;
            self.push(&[DIGITS[idx]]);
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Emit one contiguous COM1 slice atomically w.r.t. maskable IRQs.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) fn emit_critical_marker(bytes: &[u8]) {
    with_irq_masked(|| {
        #[cfg(feature = "vga_crit_mirror")]
        {
            if crate::mm::paging::is_vga_hhdm_mapped() {
                let attr = crate::drivers::vga::attr_for_marker_bytes(bytes);
                crate::drivers::vga::set_color(attr);
            }
        }
        // COM1 first, then VGA mirror: interleaved COM1+MMIO to 0xB8000 was
        // observed to stall QEMU SeaBIOS+VBE+ISO after the first mirrored byte
        // (serial stopped mid-line; NDJSON pre_marker1 still completed).
        for byte in bytes.iter() {
            boot_probe_byte(*byte);
        }
        #[cfg(feature = "vga_crit_mirror")]
        {
            if crate::mm::paging::is_vga_hhdm_mapped() {
                for byte in bytes.iter() {
                    crate::drivers::vga::write_byte(*byte);
                }
            }
        }
        // v0.4: same mirror again, into the graphical framebuffer. Separate from the VGA
        // block above because on real hardware Limine starts a graphics mode and the
        // 0xB8000 text buffer never reaches the monitor. Still after COM1, for the same
        // reason the VGA mirror is: interleaving COM1 with MMIO stalled the guest.
        #[cfg(all(not(feature = "qemu_boot"), feature = "fb_crit_mirror"))]
        {
            if crate::drivers::fbcon::is_fb_ready() {
                crate::drivers::fbcon::set_color(crate::drivers::fbcon::color_for_marker_bytes(bytes));
                crate::drivers::fbcon::write_bytes(bytes);
            }
        }
    });
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
pub(crate) fn emit_critical_marker(bytes: &[u8]) {
    for byte in bytes.iter() {
        boot_probe_byte(*byte);
    }
}
