//! Syscall ABI v1 dispatcher.
//!
//! Implements 10 numbered syscalls per `docs/SYSCALL_ABI.md`. The
//! interrupt-gate plumbing lives in `interrupts.rs::syscall_isr`; this
//! module owns the dispatch table and per-syscall behavior.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

pub const SYS_LOG: u64 = 0;
pub const SYS_COMPUTE: u64 = 1;
pub const SYS_ALLOC: u64 = 2;
pub const SYS_YIELD: u64 = 3;
pub const SYS_EXIT: u64 = 4;
pub const SYS_READ_AUDIT: u64 = 5;
pub const SYS_SET_POLICY: u64 = 6;
pub const SYS_OPEN: u64 = 7;
pub const SYS_READ: u64 = 8;
pub const SYS_WRITE: u64 = 9;

/// Syscall return value indicating "not implemented" / generic failure.
pub const SYSCALL_ERR: u64 = u64::MAX;

/// Maximum bytes a single `zpl_log` syscall is allowed to emit. Caps the
/// damage if user space sends a wild pointer.
const MAX_LOG_LEN: u64 = 4096;

#[derive(Debug, Clone, Copy)]
pub struct SyscallArgs {
    pub num: u64,
    pub a0: u64,
    pub a1: u64,
    pub a2: u64,
}

/// Dispatch the syscall identified by `num`. Returns the value to put in
/// `RAX` on the user-facing iretq.
pub fn dispatch(args: SyscallArgs) -> u64 {
    match args.num {
        SYS_LOG => sys_log(args.a0, args.a1),
        SYS_COMPUTE => sys_compute(args.a0, args.a1),
        SYS_ALLOC => sys_stub(SYS_ALLOC),
        SYS_YIELD => sys_stub(SYS_YIELD),
        SYS_EXIT => sys_exit(args.a0),
        SYS_READ_AUDIT => sys_stub(SYS_READ_AUDIT),
        SYS_SET_POLICY => sys_stub(SYS_SET_POLICY),
        SYS_OPEN => sys_open(args.a0, args.a1),
        SYS_READ => sys_read(args.a0, args.a1, args.a2),
        SYS_WRITE => sys_write(args.a0, args.a1, args.a2),
        _ => {
            write_com1_str(b"[ZPL-SYSCALL] unknown num=");
            write_com1_dec(args.num);
            write_com1_str(b"\n");
            SYSCALL_ERR
        }
    }
}

/// `SYS_COMPUTE` — read a 24-byte `ComputeInput` from user memory at
/// `in_ptr`, run whichever policy this build links, emit a
/// `[ZPL-SCHED tid=user ain=NN action=...]` marker on COM1 and (when
/// `out_ptr != 0`) write the 32-byte `ComputeOutput` back.
///
/// Return value packs `decision` into bits 8..15 and `ain_pct` into bits
/// 0..7 so user space can decode the outcome from `RAX`:
///   - `decision = 0` (Allow)  ain_pct in bits 0..7
///   - `decision = 1` (Degrade)
///   - `decision = 2` (Block)
fn sys_compute(in_ptr: u64, out_ptr: u64) -> u64 {
    if in_ptr == 0 {
        return SYSCALL_ERR;
    }
    let input = unsafe { read_compute_input(in_ptr) };
    let output = crate::zpl_policy::policy_compute(input);
    let policy = crate::zpl_policy::PolicyConfig::pb_core_default();
    let decision = crate::zpl_policy::evaluate(policy, &output);
    let ain_pct = scale_ain_to_pct(output.ain);
    let decision_code: u64 = match decision {
        crate::zpl_policy::Decision::Allow => 0,
        crate::zpl_policy::Decision::Degrade => 1,
        crate::zpl_policy::Decision::Block => 2,
    };

    // One call: the panel row and the serial line, from the same arguments. Two calls
    // is how they came to show different numbers for the same request.
    crate::sched::sched_marker::record(
        crate::ui::gate_panel::Site::SysCompute,
        crate::ui::gate_panel::Who::Ring3User,
        b"user",
        ain_pct,
        decision,
    );

    if out_ptr != 0 {
        unsafe { write_compute_output(out_ptr, &output) };
    }

    (decision_code << 8) | (ain_pct as u64 & 0xFF)
}

unsafe fn read_compute_input(ptr: u64) -> crate::zpl_policy::ComputeInput {
    // SAFETY: `ptr` lives in user-mapped memory; we read 24 bytes via
    // `read_unaligned` which never produces UB regardless of alignment.
    unsafe { core::ptr::read_unaligned(ptr as *const crate::zpl_policy::ComputeInput) }
}

unsafe fn write_compute_output(ptr: u64, out: &crate::zpl_policy::ComputeOutput) {
    unsafe {
        core::ptr::write_unaligned(ptr as *mut crate::zpl_policy::ComputeOutput, *out);
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

fn sys_open(path_ptr: u64, _flags: u64) -> u64 {
    // Read NUL-terminated path (max ramfs::MAX_PATH bytes) from user
    // memory. Trust the caller per `docs/SYSCALL_ABI.md` v1.
    if path_ptr == 0 {
        return SYSCALL_ERR;
    }
    let mut buf = [0u8; crate::fs::ramfs::MAX_PATH];
    let mut len = 0usize;
    unsafe {
        while len < crate::fs::ramfs::MAX_PATH {
            let byte = core::ptr::read_volatile((path_ptr + len as u64) as *const u8);
            if byte == 0 {
                break;
            }
            buf[len] = byte;
            len += 1;
        }
    }
    match crate::fs::ramfs::open(&buf[..len]) {
        Ok(fd) => fd as u64,
        Err(_) => SYSCALL_ERR,
    }
}

fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 || len == 0 {
        return 0;
    }
    let mut tmp = [0u8; crate::fs::ramfs::MAX_DATA];
    let cap = core::cmp::min(len as usize, tmp.len());
    let n = match crate::fs::ramfs::read(fd as usize, &mut tmp[..cap]) {
        Ok(v) => v,
        Err(_) => return SYSCALL_ERR,
    };
    unsafe {
        for i in 0..n {
            core::ptr::write_volatile((buf_ptr + i as u64) as *mut u8, tmp[i]);
        }
    }
    n as u64
}

fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 || len == 0 {
        return 0;
    }
    let mut tmp = [0u8; crate::fs::ramfs::MAX_DATA];
    let cap = core::cmp::min(len as usize, tmp.len());
    unsafe {
        for i in 0..cap {
            tmp[i] = core::ptr::read_volatile((buf_ptr + i as u64) as *const u8);
        }
    }
    match crate::fs::ramfs::write(fd as usize, &tmp[..cap]) {
        Ok(n) => n as u64,
        Err(_) => SYSCALL_ERR,
    }
}

fn sys_log(ptr: u64, len: u64) -> u64 {
    if len == 0 || len > MAX_LOG_LEN {
        return SYSCALL_ERR;
    }
    write_com1_str(b"[ZPL-SYSCALL] log: ");
    unsafe {
        let mut i: u64 = 0;
        while i < len {
            // SAFETY: `ptr` is a user-mode address; the user code lives in
            // the identity-mapped 4 KiB user region so the kernel reads it
            // through the same physical backing. v1 trusts the caller per
            // `docs/SYSCALL_ABI.md` until copy_from_user is added.
            let byte = core::ptr::read_volatile((ptr + i) as *const u8);
            write_com1_byte(byte);
            i += 1;
        }
    }
    write_com1_str(b"\n");

    // While the demo programs are running, each announcement holds the screen for
    // a moment. Without it all three programs finish between two frames and a
    // reader sees only the end state. Pacing for a demonstration, nothing more:
    // it is gone the instant the demo is done, and no other caller is affected.
    #[cfg(all(feature = "qemu_boot", feature = "vga_crit_mirror"))]
    if crate::demo::demo_programs::is_active() {
        crate::demo::demo_programs::pace();
    }

    0
}

fn sys_exit(code: u64) -> ! {
    write_com1_str(b"[ZPL-SYSCALL] exit code=");
    write_com1_dec(code);
    write_com1_str(b"\n");

    // A program the prompt started goes back to the prompt. This is checked first
    // because it is the narrowest case: one program, started by hand, with a kernel
    // stack of its own -- see `user_run`. Everything else keeps the old behaviour.
    #[cfg(any(feature = "elf_demo", feature = "user_programs"))]
    if crate::demo::user_run::is_running() {
        // SAFETY: `is_running` is true, so `user_run::enter` is on the kernel stack
        // below this call and the resume address it wrote is still there.
        unsafe { crate::demo::user_run::resume(code) };
    }

    // An exit from the demo programs means the boot is finished, not that the
    // machine should stop: the kernel still has a halt loop to enter and a panel
    // to leave on the screen. Every other caller keeps the old behaviour.
    #[cfg(all(feature = "qemu_boot", feature = "vga_crit_mirror"))]
    if crate::demo::demo_programs::is_active() {
        crate::demo::demo_programs::finish();
    }

    write_com1_str(b"[ZPL-RING] qemu_exit=success\n");
    crate::qemu_exit::qemu_exit_success();
}

fn sys_stub(num: u64) -> u64 {
    write_com1_str(b"[ZPL-SYSCALL] stub num=");
    write_com1_dec(num);
    write_com1_str(b"\n");
    0
}

/// Every `[ZPL-...]` marker this module emits, on the one path the kernel watches.
///
/// It used to be a private loop straight onto port 0x3F8, with a VGA mirror of its own.
/// That had a consequence nobody had measured until the panel contradicted itself: the
/// running summary watches the bytes passing through `boot::boot_probe_byte`, and these
/// did not pass through it. So the fingerprint on screen was the fingerprint of the log
/// *minus this module's lines* -- while the module's own documentation said the value
/// could be recomputed from the serial log with a published filter.
///
/// Measured on one ISO boot, before: recomputing from the log gave `e5ebc5b32c902a94`,
/// the screen said `6b3d394289927b14`, and dropping exactly the two lines emitted here
/// reproduced the screen value to the bit. Not a rounding difference -- two whole lines.
///
/// After, on the same four typed commands: both are `5eb0968d32ce03df`.
fn write_com1_str(bytes: &[u8]) {
    crate::drivers::console::emit_critical_marker(bytes);
}

/// One byte, same path. `write_com1_dec` and `sys_log` build their output a byte at a
/// time, and the watcher buffers until the newline, so a byte at a time is fine here.
fn write_com1_byte(byte: u8) {
    crate::drivers::console::emit_critical_marker(core::slice::from_ref(&byte));
}

fn write_com1_dec(mut value: u64) {
    if value == 0 {
        write_com1_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut idx = buf.len();
    while value > 0 {
        idx -= 1;
        buf[idx] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    for byte in buf[idx..].iter() {
        write_com1_byte(*byte);
    }
}
