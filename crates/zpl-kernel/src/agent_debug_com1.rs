//! COM1-only NDJSON breadcrumbs for physical vs QEMU bring-up (debug session).
//! Does **not** touch VGA — isolates page faults / scroll in `emit_critical_marker` path.
//!
//! Capture serial to workspace `debug-eadfc1.log` (see PHYSICAL_TEST_GUIDE / reproduction steps).

// #region agent log
#[inline]
fn com1_raw_byte(byte: u8) {
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

/// Same IF-restore semantics as `boot::with_irq_masked` (avoid unconditional `sti`).
pub fn with_irq_masked_com1<R>(f: impl FnOnce() -> R) -> R {
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

/// One NDJSON line to COM1 only (no VGA).
pub fn ndjson_line(bytes: &[u8]) {
    with_irq_masked_com1(|| {
        for &b in bytes {
            com1_raw_byte(b);
        }
    });
}

pub const DBG_PRE_MARKER1: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H1\",\"location\":\"pre_marker1\",\"message\":\"com1_before_first_vga_emit\"}\n";
pub const DBG_POST_MARKER1: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H1\",\"location\":\"post_marker1\",\"message\":\"after_first_emit_critical\"}\n";
pub const DBG_POST_SERIAL: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H4\",\"location\":\"post_serial\",\"message\":\"after_init_early_serial\"}\n";
pub const DBG_POST_PAGING_SC: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H4\",\"location\":\"post_paging_selfcheck\",\"message\":\"before_interrupts_init\"}\n";
pub const DBG_POST_IRQ_INIT: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H3\",\"location\":\"post_interrupts_init\",\"message\":\"after_sti\"}\n";
pub const DBG_POST_HAL: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H4\",\"location\":\"post_hal\",\"message\":\"after_hal_selfcheck\"}\n";
pub const DBG_PRE_VISUAL_HOLD: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H4\",\"location\":\"pre_visual_hold\",\"message\":\"halt_path_entry\"}\n";
pub const DBG_VGA_SCROLL_ENTER: &[u8] =
    b"{\"sessionId\":\"eadfc1\",\"hypothesisId\":\"H2\",\"location\":\"vga_scroll_one_line\",\"message\":\"enter\"}\n";
// #endregion agent log
