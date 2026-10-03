//! Shared memory + AIN gating.
//!
//! Each `shm_write` call is gated by the kernel `pb_core_nostd` policy:
//! the caller passes a `bias_hint` (0..1) and the kernel computes an AIN
//! score on the fly. If the resulting decision is `Block`, the write is
//! refused and a `[ZPL-SHM]` audit marker is emitted on COM1. Otherwise
//! the bytes land in the shared region and an `ALLOW` marker is logged.
//!
//! v0.1 design:
//! - `MAX_SHM` (= 4) static slots with 256-byte buffers.
//! - `shm_create(name)` allocates a slot, `shm_write(id, offset, bytes, bias)`
//!   gates the write, `shm_read(id, offset, buf)` reads with no policy
//!   check (reads are pure observation).
//!
//! Out of scope for v1: cross-process attach (waiting on ring-3 fork +
//! page-table sharing); persistent audit chain entries (#19).

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::sync::atomic::{AtomicBool, Ordering};

pub const MAX_SHM: usize = 4;
pub const SHM_NAME_LEN: usize = 32;
pub const SHM_DATA_LEN: usize = 256;

#[repr(C)]
pub struct ShmRegion {
    used: bool,
    name_len: u8,
    name: [u8; SHM_NAME_LEN],
    data: [u8; SHM_DATA_LEN],
}

impl ShmRegion {
    pub const fn new() -> Self {
        Self {
            used: false,
            name_len: 0,
            name: [0; SHM_NAME_LEN],
            data: [0; SHM_DATA_LEN],
        }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const SHM_INIT: ShmRegion = ShmRegion::new();
static mut TABLE: [ShmRegion; MAX_SHM] = [SHM_INIT; MAX_SHM];
static INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Reset the SHM table.
///
/// # Safety
/// Caller must guarantee no other thread is touching the table.
pub unsafe fn init() {
    unsafe {
        let table = (&raw mut TABLE) as *mut ShmRegion;
        for i in 0..MAX_SHM {
            *table.add(i) = ShmRegion::new();
        }
    }
    INIT_DONE.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmError {
    NotInitialized,
    NoSlot,
    BadId,
    NameTooLong,
    OutOfRange,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Allow { ain_pct: u8 },
    Degrade { ain_pct: u8 },
    Block { ain_pct: u8 },
}

fn check_init() -> Result<(), ShmError> {
    if INIT_DONE.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(ShmError::NotInitialized)
    }
}

/// Allocate a slot for `name`. Returns `shm_id` (`0..MAX_SHM`).
pub fn shm_create(name: &[u8]) -> Result<usize, ShmError> {
    check_init()?;
    if name.len() > SHM_NAME_LEN {
        return Err(ShmError::NameTooLong);
    }
    unsafe {
        let table = (&raw mut TABLE) as *mut ShmRegion;
        // A name already in use is that region, not a request for another one. This is
        // what the word "shared" means: two callers naming the same region have to reach
        // the same bytes, or there is nothing shared about it.
        //
        // It also fixes a defect a visitor would hit in the first minute. There are four
        // slots. The demo asked for a new one on every keypress and never released it,
        // so the fourth `run clean` -- or the fourth press of `1` in the v0.4 menu --
        // failed, and every one after it. The kernel said `shm-create-failed` and looked
        // broken while doing exactly what it was told.
        for i in 0..MAX_SHM {
            let slot = &*table.add(i);
            if slot.used && slot.name_len as usize == name.len()
                && slot.name[..name.len()] == *name
            {
                return Ok(i);
            }
        }
        for i in 0..MAX_SHM {
            let slot = &mut *table.add(i);
            if !slot.used {
                slot.used = true;
                slot.name_len = name.len() as u8;
                slot.name[..name.len()].copy_from_slice(name);
                slot.data.fill(0);
                return Ok(i);
            }
        }
    }
    Err(ShmError::NoSlot)
}

/// Write `bytes` at `offset` if the AIN policy allows it. Returns the
/// decision so callers can drive their own retry/back-off.
pub fn shm_write(
    shm_id: usize,
    offset: usize,
    bytes: &[u8],
    bias_hint: f64,
) -> Result<WriteOutcome, ShmError> {
    check_init()?;
    if shm_id >= MAX_SHM {
        return Err(ShmError::BadId);
    }
    if offset + bytes.len() > SHM_DATA_LEN {
        return Err(ShmError::OutOfRange);
    }

    let input = crate::zpl_policy::ComputeInput {
        bias: bias_hint,
        dimension: 9,
        samples: 64,
        seed: ((shm_id as u64) << 32) | (offset as u64),
    };
    let output = crate::zpl_policy::policy_compute(input);
    let policy = crate::zpl_policy::PolicyConfig::pb_core_default();
    let decision = crate::zpl_policy::evaluate(policy, &output);
    let ain_pct = scale_ain_pct(output.ain);

    let outcome = match decision {
        crate::zpl_policy::Decision::Allow => WriteOutcome::Allow { ain_pct },
        crate::zpl_policy::Decision::Degrade => WriteOutcome::Degrade { ain_pct },
        crate::zpl_policy::Decision::Block => WriteOutcome::Block { ain_pct },
    };

    emit_marker(shm_id, ain_pct, decision);

    if matches!(decision, crate::zpl_policy::Decision::Block) {
        return Ok(outcome);
    }

    unsafe {
        let table = (&raw mut TABLE) as *mut ShmRegion;
        let slot = &mut *table.add(shm_id);
        if !slot.used {
            return Err(ShmError::BadId);
        }
        slot.data[offset..offset + bytes.len()].copy_from_slice(bytes);
    }
    Ok(outcome)
}

/// Read `buf.len()` bytes starting at `offset` with no policy check.
pub fn shm_read(shm_id: usize, offset: usize, buf: &mut [u8]) -> Result<usize, ShmError> {
    check_init()?;
    if shm_id >= MAX_SHM {
        return Err(ShmError::BadId);
    }
    if offset + buf.len() > SHM_DATA_LEN {
        return Err(ShmError::OutOfRange);
    }
    unsafe {
        let table = (&raw mut TABLE) as *mut ShmRegion;
        let slot = &mut *table.add(shm_id);
        if !slot.used {
            return Err(ShmError::BadId);
        }
        buf.copy_from_slice(&slot.data[offset..offset + buf.len()]);
        Ok(buf.len())
    }
}

fn scale_ain_pct(ain: f64) -> u8 {
    let scaled = ain * 100.0 + 0.5;
    if scaled < 0.0 {
        0
    } else if scaled > 100.0 {
        100
    } else {
        scaled as u8
    }
}

fn emit_marker(shm_id: usize, ain_pct: u8, decision: crate::zpl_policy::Decision) {
    use crate::drivers::console::{boot_probe_byte, with_irq_masked};
    with_irq_masked(|| {
        // Built once into a buffer, then written to both sinks, so the serial line
        // and the screen line cannot drift apart. The shared-memory decisions are
        // the clearest thing the gate does -- one write allowed, one refused -- and
        // before this they reached the serial port only.
        #[cfg(any(
            all(feature = "qemu_boot", feature = "vga_crit_mirror"),
            feature = "gate_panel_fb"
        ))]
        crate::ui::gate_panel::note(
            crate::ui::gate_panel::Site::ShmWrite,
            crate::ui::gate_panel::Who::Shm0,
            ain_pct,
            decision,
        );

        let mut line = [0u8; 48];
        let mut n = 0usize;
        let push = |buf: &mut [u8; 48], n: &mut usize, bytes: &[u8]| {
            for b in bytes {
                if *n < buf.len() {
                    buf[*n] = *b;
                    *n += 1;
                }
            }
        };
        push(&mut line, &mut n, b"[ZPL-SHM] shm_id=");
        push(&mut line, &mut n, &[b'0' + (shm_id as u8)]);
        push(&mut line, &mut n, b" ain=");
        let mut dec = [0u8; 3];
        let dn = u8_to_dec(ain_pct, &mut dec);
        push(&mut line, &mut n, &dec[..dn]);
        push(&mut line, &mut n, b" action=");
        let label = match decision {
            crate::zpl_policy::Decision::Allow => b"ALLOW".as_slice(),
            crate::zpl_policy::Decision::Degrade => b"DEGRADE".as_slice(),
            crate::zpl_policy::Decision::Block => b"BLOCK".as_slice(),
        };
        push(&mut line, &mut n, label);
        push(&mut line, &mut n, b"\n");

        for b in &line[..n] {
            boot_probe_byte(*b);
        }

        #[cfg(feature = "vga_crit_mirror")]
        {
            crate::drivers::vga::set_color(crate::drivers::vga::attr_for_marker_bytes(label));
            for b in &line[..n] {
                crate::drivers::vga::write_byte(*b);
            }
        }
    });
}

/// Decimal digits of `value` into `buf`; returns how many were written.
fn u8_to_dec(value: u8, buf: &mut [u8; 3]) -> usize {
    if value >= 100 {
        buf[0] = b'0' + (value / 100) % 10;
        buf[1] = b'0' + (value / 10) % 10;
        buf[2] = b'0' + value % 10;
        3
    } else if value >= 10 {
        buf[0] = b'0' + (value / 10) % 10;
        buf[1] = b'0' + value % 10;
        2
    } else {
        buf[0] = b'0' + value;
        1
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    CreateFailed,
    AllowMissing,
    BlockMissing,
}

pub fn self_check() -> Result<(), SelfCheckErr> {
    let id = shm_create(b"observability").map_err(|_| SelfCheckErr::CreateFailed)?;
    let allow = shm_write(id, 0, b"clean", 0.5).map_err(|_| SelfCheckErr::AllowMissing)?;
    if !matches!(allow, WriteOutcome::Allow { .. }) {
        return Err(SelfCheckErr::AllowMissing);
    }
    let blocked =
        shm_write(id, 0, b"hostile", 0.95).map_err(|_| SelfCheckErr::BlockMissing)?;
    if !matches!(blocked, WriteOutcome::Block { .. }) {
        return Err(SelfCheckErr::BlockMissing);
    }

    // Asking for the same region again has to give the same region, every time.
    //
    // There are four slots. Until this was fixed, the demo asked for a new one on
    // every keypress and never released it, so the fourth press failed and so did
    // every press after it -- in the v0.4 menu and at the shell prompt both. The
    // count below is well past four on purpose: the old code did not fail on the
    // second attempt, it failed on the fifth, which is late enough to survive a
    // casual look. Running it here means the QEMU smoke and the boot gate check it
    // on every build, on the real target, which no host test can do: this module
    // only compiles bare-metal.
    for _ in 0..12 {
        let again = shm_create(b"observability").map_err(|_| SelfCheckErr::CreateFailed)?;
        if again != id {
            return Err(SelfCheckErr::CreateFailed);
        }
    }

    Ok(())
}
