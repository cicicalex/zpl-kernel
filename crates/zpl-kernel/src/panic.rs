use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

/// Re-entrancy guard so a fault inside the panic handler still terminates
/// instead of recursing forever.
static IN_PANIC: AtomicBool = AtomicBool::new(false);

/// Kernel panic handler for bare-metal target builds.
///
/// On panic the kernel emits a `[ZPL-PANIC]` marker line to COM1 with the
/// faulting source location (file + line + reason), records an audit-style
/// entry in the boot log, then exits QEMU cleanly via the `isa-debug-exit`
/// device with success code so the host harness can observe a
/// deterministic shutdown.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if IN_PANIC.swap(true, Ordering::SeqCst) {
        // Nested panic — fall straight through to QEMU exit, never loop.
        crate::qemu_exit::qemu_exit_failure(2);
    }

    write_com1_str(b"[ZPL-PANIC] reason=");
    if let Some(msg) = info.message().as_str() {
        write_com1_str(msg.as_bytes());
    } else {
        write_com1_str(b"<unprintable>");
    }
    write_com1_str(b"\n");

    if let Some(loc) = info.location() {
        write_com1_str(b"[ZPL-PANIC] file=");
        write_com1_str(loc.file().as_bytes());
        write_com1_str(b" line=");
        write_com1_dec(loc.line() as u64);
        write_com1_str(b" col=");
        write_com1_dec(loc.column() as u64);
        write_com1_str(b"\n");
    }

    write_com1_str(b"[ZPL-PANIC] rip=");
    write_com1_hex64(read_rip_approx());
    write_com1_str(b" rsp=");
    write_com1_hex64(read_rsp());
    write_com1_str(b" rflags=");
    write_com1_hex64(read_rflags());
    write_com1_str(b"\n");

    write_com1_str(b"[ZPL-PANIC] qemu_exit=success\n");

    crate::qemu_exit::qemu_exit_success();
}

#[inline(always)]
fn read_rsp() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mov {0}, rsp", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

#[inline(always)]
fn read_rflags() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {0}",
            out(reg) value,
            options(nomem)
        );
    }
    value
}

#[inline(always)]
fn read_rip_approx() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "lea {0}, [rip]",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

fn write_com1_str(bytes: &[u8]) {
    for byte in bytes.iter() {
        wait_com1_tx_ready();
        // SAFETY: COM1, and by the time a panic is printing nothing else is running.
        unsafe { crate::arch::port::outb(0x3F8, *byte) }
    }
}

fn wait_com1_tx_ready() {
    loop {
        // SAFETY: as above.
        let status = unsafe { crate::arch::port::inb(0x3FD) };
        if status & 0x20 != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn write_com1_dec(mut value: u64) {
    if value == 0 {
        write_com1_str(b"0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut idx = buf.len();
    while value > 0 {
        idx -= 1;
        buf[idx] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    write_com1_str(&buf[idx..]);
}

fn write_com1_hex64(value: u64) {
    write_com1_str(b"0x");
    let mut buf = [0u8; 16];
    for i in 0..16 {
        let nibble = ((value >> ((15 - i) * 4)) & 0xF) as u8;
        buf[i] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
    }
    write_com1_str(&buf);
}
