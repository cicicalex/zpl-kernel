//! Three tiny ring-3 programs that exercise the gate, one per rule.
//!
//! The boot already proved the kernel *can* reach ring 3 (`ring3_demo`, one
//! program that logs and exits). What it never showed was the point of the
//! thing: a program in user space asking for something and being answered.
//! These three do that, each through a real `int 0x80`, each landing on a
//! different one of the demo policy's three rules:
//!
//! | program | what it asks for | rule | verdict |
//! |---|---|---|---|
//! | 1 `clean`    | one modest request                 | 1 within the budget  | `ALLOW` |
//! | 2 `hostile`  | a request far over the limit       | 2 over the limit     | `BLOCK` |
//! | 3 `flood`    | the same modest request, 400 times | 3 past the quota     | `ALLOW` then `DEGRADE` |
//!
//! Program 3 is the only one that needs a loop: the per-boot quota has not been
//! reached by the time the boot gets here, so the program has to reach it
//! itself. That is exactly what "asking too often" means, and the screen shows
//! the switch happening — green lines, then yellow ones.
//!
//! **They are three programs, not three processes.** There is no `fork` yet, so
//! they share one address space and run one after another from a four-instruction
//! runner. The claim is only what the ABI supports: ring 3, `int 0x80`, and a
//! verdict per request.
//!
//! The machine code is written out by hand below rather than assembled, for the
//! same reason `ring3_demo` does it: the kernel cannot load an ELF from user
//! space before it has a filesystem to load it from, and these have to run before
//! the kernel idles.
//!
//! Compiled only for the public `-kernel` build, the same configuration the
//! policy panel is compiled for.

#![cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "qemu_boot",
    feature = "vga_crit_mirror"
))]

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::mm::frame_alloc::alloc_frame;
use crate::mm::paging;

const USER_CODE_VIRT: u64 = paging::USER_REGION_BASE + 0x10_000;
const USER_DATA_VIRT: u64 = paging::USER_REGION_BASE + 0x10_800;
const USER_STACK_VIRT_BASE: u64 = paging::USER_REGION_BASE + 0x20_000;
const USER_STACK_TOP: u64 = USER_STACK_VIRT_BASE + 0x1000;

/// Offset of the hostile request inside the data page. One `ComputeInput` is 24
/// bytes; the second one starts right after the first.
const HOSTILE_OFFSET: u64 = 24;

const USER_CODE_SELECTOR_DPL3: u64 = 0x18 | 3;
const USER_DATA_SELECTOR_DPL3: u64 = 0x20 | 3;
const USER_RFLAGS: u64 = 0x202; // IF=1, reserved bit 1 set

/// How many requests program 3 makes. The quota is 256 per boot and the boot has
/// spent part of it by now, so this has to be comfortably more than what is left.
const FLOOD_REQUESTS: u32 = 400;

/// Set while the demo is running, so `sys_exit` knows the exit came from these
/// programs and finishes the boot instead of shutting the machine down.
static DEMO_ACTIVE: AtomicBool = AtomicBool::new(false);

#[must_use]
pub fn is_active() -> bool {
    DEMO_ACTIVE.load(Ordering::Relaxed)
}

/// Where each program's announcement sits in the data page, and how long it is.
///
/// Each program says what it is about to ask for before it asks, so a reader
/// watching the screen sees the request and the verdict next to each other
/// instead of a verdict on its own.
const MSG1: &[u8] = b"program 1 (clean): one modest request";
const MSG2: &[u8] = b"program 2 (hostile): asks far over the limit";
const MSG3: &[u8] = b"program 3 (flood): asks 400 times, hits the quota";
const MSG1_OFFSET: u64 = 0x40;
const MSG2_OFFSET: u64 = 0x80;
const MSG3_OFFSET: u64 = 0xC0;

/// The three programs, in machine code, laid out end to end.
///
/// ```text
///     mov eax, 0 / mov edi, <msg1> / mov esi, 37 / int 0x80   ; SYS_LOG
///     mov eax, 1 / mov edi, <clean> / mov esi, 0 / int 0x80   ; SYS_COMPUTE  -> ALLOW
///     mov eax, 0 / mov edi, <msg2> / mov esi, 44 / int 0x80
///     mov eax, 1 / mov edi, <hostile> / mov esi, 0 / int 0x80 ; SYS_COMPUTE  -> BLOCK
///     mov eax, 0 / mov edi, <msg3> / mov esi, 49 / int 0x80
///     mov ecx, 400
///   again:
///     push rcx
///     mov eax, 1 / mov edi, <clean> / mov esi, 0 / int 0x80   ; ALLOW, then DEGRADE
///     pop rcx
///     dec ecx
///     jnz again                                               ; rel8 = -23
///     mov eax, 4 / mov edi, 0 / int 0x80                      ; SYS_EXIT
/// ```
///
/// After program 2 is refused, the program does not then attempt the write: the
/// point of a gate is that the answer is acted on, not that it is logged.
///
/// `rcx` is saved around the syscall even though the `x86-interrupt` ABI makes
/// the handler preserve what it touches. The loop counter is the one value here
/// that a mistake would turn into an endless loop in ring 3, and there is no way
/// out of that one.
///
/// The bytes were assembled and the jump target checked before they were written
/// down; the `const` assertions below keep the three numbers that appear twice
/// from drifting apart afterwards.
const PROGRAMS: [u8; 125] = [
    0xB8, 0x00, 0x00, 0x00, 0x00, 0xBF, 0x40, 0x08,
    0x01, 0x40, 0xBE, 0x25, 0x00, 0x00, 0x00, 0xCD,
    0x80, 0xB8, 0x01, 0x00, 0x00, 0x00, 0xBF, 0x00,
    0x08, 0x01, 0x40, 0xBE, 0x00, 0x00, 0x00, 0x00,
    0xCD, 0x80, 0xB8, 0x00, 0x00, 0x00, 0x00, 0xBF,
    0x80, 0x08, 0x01, 0x40, 0xBE, 0x2C, 0x00, 0x00,
    0x00, 0xCD, 0x80, 0xB8, 0x01, 0x00, 0x00, 0x00,
    0xBF, 0x18, 0x08, 0x01, 0x40, 0xBE, 0x00, 0x00,
    0x00, 0x00, 0xCD, 0x80, 0xB8, 0x00, 0x00, 0x00,
    0x00, 0xBF, 0xC0, 0x08, 0x01, 0x40, 0xBE, 0x31,
    0x00, 0x00, 0x00, 0xCD, 0x80, 0xB9, 0x90, 0x01,
    0x00, 0x00, 0x51, 0xB8, 0x01, 0x00, 0x00, 0x00,
    0xBF, 0x00, 0x08, 0x01, 0x40, 0xBE, 0x00, 0x00,
    0x00, 0x00, 0xCD, 0x80, 0x59, 0xFF, 0xC9, 0x75,
    0xE9, 0xB8, 0x04, 0x00, 0x00, 0x00, 0xBF, 0x00,
    0x00, 0x00, 0x00, 0xCD, 0x80,
];

// Hand-written machine code is only as good as the numbers inside it, and seven of
// them appear twice -- once as a Rust constant and once as little-endian bytes.
// These make the compiler compare the two, so changing one and not the other is a
// build error rather than a program that quietly asks for the wrong thing.
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[23], PROGRAMS[24], PROGRAMS[25], PROGRAMS[26]]) as u64
        == USER_DATA_VIRT,
    "program 1 points somewhere other than the clean request"
);
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[57], PROGRAMS[58], PROGRAMS[59], PROGRAMS[60]]) as u64
        == USER_DATA_VIRT + HOSTILE_OFFSET,
    "program 2 points somewhere other than the hostile request"
);
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[86], PROGRAMS[87], PROGRAMS[88], PROGRAMS[89]])
        == FLOOD_REQUESTS,
    "the loop count in the machine code and FLOOD_REQUESTS have drifted apart"
);
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[97], PROGRAMS[98], PROGRAMS[99], PROGRAMS[100]]) as u64
        == USER_DATA_VIRT,
    "program 3 points somewhere other than the clean request"
);
// Each announcement carries its length in the `mov esi` right before its syscall. A
// message edited without editing that number would print a truncated line, or read
// past the end of the string.
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[11], PROGRAMS[12], PROGRAMS[13], PROGRAMS[14]]) as usize
        == MSG1.len(),
    "the length program 1 passes and MSG1 have drifted apart"
);
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[45], PROGRAMS[46], PROGRAMS[47], PROGRAMS[48]]) as usize
        == MSG2.len(),
    "the length program 2 passes and MSG2 have drifted apart"
);
const _: () = assert!(
    u32::from_le_bytes([PROGRAMS[79], PROGRAMS[80], PROGRAMS[81], PROGRAMS[82]]) as usize
        == MSG3.len(),
    "the length program 3 passes and MSG3 have drifted apart"
);
// Everything the programs touch stays inside the one 4 KiB page they were given.
const _: () = assert!(
    MSG3_OFFSET as usize + MSG3.len() < 0x800,
    "the announcements have grown past the data area"
);

#[derive(Debug, Clone, Copy)]
pub enum DemoErr {
    NoCodeFrame,
    NoStackFrame,
    NoDataFrame,
    MapFailed,
}

/// The two requests the programs make, written into the user data page.
///
/// `bias` is the one number a request declares. The demo reads it as "how far
/// this request pushes", and scores it `1 - bias`; nothing else in the input
/// affects the demo's answer.
fn requests() -> [crate::zpl_policy::ComputeInput; 2] {
    [
        crate::zpl_policy::ComputeInput {
            bias: 0.05, // a modest request: scores 0.95, well inside the budget
            dimension: 9,
            samples: 64,
            seed: 1,
        },
        crate::zpl_policy::ComputeInput {
            bias: 0.95, // far over: scores 0.05, below the block line
            dimension: 9,
            samples: 64,
            seed: 2,
        },
    ]
}

/// Map the user pages and write the programs and their data into them.
///
/// # Safety
/// Paging and the frame allocator must be initialised, and the TSS must already
/// carry a valid `rsp0`, because the first `int 0x80` switches stacks onto it.
unsafe fn prepare() -> Result<(), DemoErr> {
    let code_phys = alloc_frame().ok_or(DemoErr::NoCodeFrame)?;
    let stack_phys = alloc_frame().ok_or(DemoErr::NoStackFrame)?;

    unsafe {
        paging::map_4k_user_page(USER_CODE_VIRT, code_phys).map_err(|_| DemoErr::MapFailed)?;
        paging::map_4k_user_page(USER_STACK_VIRT_BASE, stack_phys)
            .map_err(|_| DemoErr::MapFailed)?;
    }

    // The data lives in the same 4 KiB page as the code (0x800 in), so it needs no
    // frame of its own. Asserted rather than assumed: if the layout ever moves the
    // data past the page boundary this stops instead of writing into nothing.
    if USER_DATA_VIRT + HOSTILE_OFFSET + 24 > USER_CODE_VIRT + 0x1000 {
        return Err(DemoErr::NoDataFrame);
    }

    unsafe {
        let code = USER_CODE_VIRT as *mut u8;
        for (i, byte) in PROGRAMS.iter().enumerate() {
            core::ptr::write_volatile(code.add(i), *byte);
        }
        let data = USER_DATA_VIRT as *mut crate::zpl_policy::ComputeInput;
        for (i, req) in requests().iter().enumerate() {
            core::ptr::write_unaligned(data.add(i), *req);
        }
        for (offset, msg) in [
            (MSG1_OFFSET, MSG1),
            (MSG2_OFFSET, MSG2),
            (MSG3_OFFSET, MSG3),
        ] {
            let dst = (USER_DATA_VIRT + offset) as *mut u8;
            for (i, byte) in msg.iter().enumerate() {
                core::ptr::write_volatile(dst.add(i), *byte);
            }
        }
    }
    Ok(())
}

/// Hold the screen for roughly three quarters of a second.
///
/// The three programs run in microseconds, which on a screen means all of it
/// happens between two frames and a reader sees only the end state. The kernel
/// pauses on each program's announcement so the request and its verdict can be
/// read as they land.
///
/// **This is pacing for a demonstration and nothing else.** It runs only while
/// the demo programs are active, it is a busy-wait rather than a timer so it
/// needs no new machinery, and interrupts stay on throughout -- the scheduler
/// keeps ticking and the panel keeps counting while it waits.
pub fn pace() {
    // Deliberately a floor, not a calibration: on a faster part this waits less
    // wall-clock time, which is the harmless direction to be wrong in.
    const MIN_TSC_HZ: u64 = 1_500_000_000;
    let ticks = MIN_TSC_HZ / 4 * 3;
    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    while unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(t0) < ticks {
        core::hint::spin_loop();
    }
}

/// Run the three programs in ring 3, then finish the boot.
///
/// Never returns: the programs end with `SYS_EXIT`, which lands in
/// [`crate::syscall`], sees [`is_active`] and calls [`finish`] from there rather
/// than shutting the machine down.
pub fn run_then_halt() -> ! {
    static MARKER_START: &[u8] = b"[ZPL-RING3] demo programs: clean, hostile, flood\n";
    static MARKER_FAIL: &[u8] = b"[ZPL-RING3] demo programs prepare FAIL\n";

    if unsafe { prepare() }.is_err() {
        crate::drivers::console::emit_critical_marker(MARKER_FAIL);
        finish();
    }
    crate::drivers::console::emit_critical_marker(MARKER_START);
    DEMO_ACTIVE.store(true, Ordering::Relaxed);

    unsafe {
        asm!(
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss = in(reg) USER_DATA_SELECTOR_DPL3,
            rsp = in(reg) USER_STACK_TOP,
            rflags = in(reg) USER_RFLAGS,
            cs = in(reg) USER_CODE_SELECTOR_DPL3,
            rip = in(reg) USER_CODE_VIRT,
            options(noreturn),
        );
    }
}

/// What the boot does once the demo programs are done.
///
/// Reached from inside the `int 0x80` handler, so it runs on the interrupt stack
/// rather than the one `kernel_entry` was using. That stack is abandoned on the
/// way into ring 3 and never returned to, which is what makes this safe: there is
/// nothing left on it to corrupt.
pub fn finish() -> ! {
    static MARKER_DONE: &[u8] = b"[ZPL-RING3] demo programs done\n";
    static MARKER_HALT: &[u8] = b"[ZPL-BOOT] halt loop entered\n";

    DEMO_ACTIVE.store(false, Ordering::Relaxed);
    crate::drivers::console::emit_critical_marker(MARKER_DONE);
    crate::audit::timing::checkpoint(crate::audit::timing::Phase::HaltLoop);
    crate::drivers::console::with_irq_masked(|| {
        crate::audit::timing::emit(crate::drivers::console::boot_probe_byte);
    });
    // The last thing before the boot stops: ask the network its question. The
    // card was armed near the top of the boot, and everything since has been
    // time its answer did not have to wait for.
    crate::boot::run_e1000_ask();

    crate::drivers::console::emit_critical_marker(MARKER_HALT);
    crate::ui::gate_panel::draw();

    // The prompt, where this build has one. `halt_once` waits for an interrupt, and
    // the timer fires often enough that a keystroke is read within a tick -- but the
    // shell is also polled before the halt, so a character typed while the CPU was
    // awake is not left sitting in the controller.
    #[cfg(feature = "shell")]
    {
        crate::ui::shell::kernel::seed_programs();
        crate::ui::shell::kernel::banner();
    }

    loop {
        #[cfg(feature = "shell")]
        {
            crate::ui::shell::kernel::poll();
            // `hlt` waits for an interrupt, so with IF clear it never wakes: the
            // prompt prints once and then ignores every key for ever. This path can
            // arrive with interrupts off, and the same mistake cost an hour on the
            // network console earlier -- so it is checked rather than assumed.
            let flags: u64;
            // SAFETY: reading RFLAGS has no side effects.
            unsafe {
                core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, nostack));
            }
            if flags & (1 << 9) == 0 {
                core::hint::spin_loop();
                continue;
            }
        }
        crate::arch::x86_64::halt_once();
    }
}
