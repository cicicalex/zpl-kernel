//! In-kernel rdtsc-based phase profiler (closes the deferred half of
//! task `#35` performance benchmarks).
//!
//! Boot phases call `checkpoint(phase)` to record an `rdtsc` value
//! into a fixed slot. After all phases finish the boot driver calls
//! `emit(boot_probe_byte)` which writes a per-phase
//! `[ZPL-PERF phase=N name=... rdtsc=... delta=...]` line on COM1.
//!
//! Why a fixed array of slots rather than a `Vec`? The profiler runs
//! on the bare-metal boot path before the heap allocator is
//! initialised; static storage keeps the dependency arrow short.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Number of slots reserved in the static checkpoint array. Adding
/// a new boot phase = adding a new variant + bumping this constant.
pub const MAX_PHASES: usize = 16;

/// Logical boot-phase tags. Variant order is the temporal order of
/// the corresponding `checkpoint(...)` calls during boot.
#[derive(Debug, Clone, Copy)]
pub enum Phase {
    KernelEntry = 0,
    SerialReady = 1,
    FrameAlloc = 2,
    Heap = 3,
    AuditChain = 4,
    Smp = 5,
    Paging = 6,
    Ring3 = 7,
    Ramfs = 8,
    Process = 9,
    Ipc = 10,
    Shm = 11,
    Matrix = 12,
    Runqueue = 13,
    Hal = 14,
    HaltLoop = 15,
}

impl Phase {
    pub fn name(self) -> &'static [u8] {
        match self {
            Phase::KernelEntry => b"kernel_entry",
            Phase::SerialReady => b"serial_ready",
            Phase::FrameAlloc => b"frame_alloc",
            Phase::Heap => b"heap",
            Phase::AuditChain => b"audit_chain",
            Phase::Smp => b"smp",
            Phase::Paging => b"paging",
            Phase::Ring3 => b"ring3",
            Phase::Ramfs => b"ramfs",
            Phase::Process => b"process",
            Phase::Ipc => b"ipc",
            Phase::Shm => b"shm",
            Phase::Matrix => b"matrix",
            Phase::Hal => b"hal",
            Phase::Runqueue => b"runqueue",
            Phase::HaltLoop => b"halt_loop",
        }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);

static SLOTS: [AtomicU64; MAX_PHASES] = [ZERO; MAX_PHASES];

#[inline]
pub fn rdtsc() -> u64 {
    let mut hi: u32;
    let mut lo: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

pub fn checkpoint(phase: Phase) {
    let now = rdtsc();
    SLOTS[phase as usize].store(now, Ordering::SeqCst);
}

/// Emit one `[ZPL-PERF phase=N name=... rdtsc=... delta=...]` line
/// per recorded slot via the supplied byte sink. `delta` is the
/// rdtsc difference vs. the previous recorded slot; for the first
/// recorded slot delta is `0`.
pub fn emit<F: FnMut(u8)>(mut sink: F) {
    let mut prev: Option<u64> = None;
    for idx in 0..MAX_PHASES {
        let v = SLOTS[idx].load(Ordering::SeqCst);
        if v == 0 {
            continue;
        }
        let delta = match prev {
            Some(p) => v.saturating_sub(p),
            None => 0,
        };
        prev = Some(v);
        let phase = phase_from_index(idx);
        emit_line(&mut sink, idx, phase, v, delta);
    }
}

fn phase_from_index(idx: usize) -> Phase {
    match idx {
        0 => Phase::KernelEntry,
        1 => Phase::SerialReady,
        2 => Phase::FrameAlloc,
        3 => Phase::Heap,
        4 => Phase::AuditChain,
        5 => Phase::Smp,
        6 => Phase::Paging,
        7 => Phase::Ring3,
        8 => Phase::Ramfs,
        9 => Phase::Process,
        10 => Phase::Ipc,
        11 => Phase::Shm,
        12 => Phase::Matrix,
        13 => Phase::Runqueue,
        14 => Phase::Hal,
        _ => Phase::HaltLoop,
    }
}

fn emit_line<F: FnMut(u8)>(sink: &mut F, idx: usize, phase: Phase, rdtsc_val: u64, delta: u64) {
    for b in b"[ZPL-PERF phase=" {
        sink(*b);
    }
    emit_u32_dec(sink, idx as u32);
    for b in b" name=" {
        sink(*b);
    }
    for b in phase.name() {
        sink(*b);
    }
    for b in b" rdtsc=" {
        sink(*b);
    }
    emit_u64_dec(sink, rdtsc_val);
    for b in b" delta=" {
        sink(*b);
    }
    emit_u64_dec(sink, delta);
    sink(b']');
    sink(b'\n');
}

fn emit_u32_dec<F: FnMut(u8)>(sink: &mut F, value: u32) {
    emit_u64_dec(sink, value as u64);
}

fn emit_u64_dec<F: FnMut(u8)>(sink: &mut F, mut value: u64) {
    if value == 0 {
        sink(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut idx = 0;
    while value > 0 {
        buf[idx] = b'0' + (value % 10) as u8;
        value /= 10;
        idx += 1;
    }
    while idx > 0 {
        idx -= 1;
        sink(buf[idx]);
    }
}
