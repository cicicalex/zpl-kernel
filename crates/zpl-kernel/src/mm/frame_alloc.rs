//! Bitmap-based physical frame allocator for the first 1 GiB of identity-
//! mapped physical memory.
//!
//! Layout:
//! - 1 frame = `FRAME_SIZE` bytes (4 KiB).
//! - 1 GiB / 4 KiB = `TOTAL_FRAMES` (= 262_144) frames tracked.
//! - Bitmap is a static array of `BITMAP_WORDS` (= 4096) `AtomicU64`s.
//!   Bit `1` means "frame in use".
//!
//! Concurrency:
//! - Each word is mutated via `compare_exchange_weak`; `free_frame` uses
//!   `fetch_and`. No global lock. Safe for a future SMP bring-up.

use core::sync::atomic::{AtomicU64, Ordering};

pub const FRAME_SIZE: u64 = 4096;
pub const TOTAL_FRAMES: usize = 262_144;
const BITMAP_WORDS: usize = TOTAL_FRAMES / 64;

static BITMAP: [AtomicU64; BITMAP_WORDS] = {
    // Canonical no_std array-of-atomic init: the const helper is only used as
    // a const initializer expression for the array repeat below. It is never
    // shared as a "constant" with interior mutability that callers can copy.
    #[allow(clippy::declare_interior_mutable_const)]
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; BITMAP_WORDS]
};

static BASE_PHYS: AtomicU64 = AtomicU64::new(0);

/// How many usable regions the boot loader may describe. Limine reports a dozen or so
/// on an ordinary machine; anything past this is ignored, which costs memory and never
/// costs correctness, because ignoring a region means not allocating from it.
pub const MAX_USABLE_RANGES: usize = 24;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);
static USABLE_START: [AtomicU64; MAX_USABLE_RANGES] = [ZERO; MAX_USABLE_RANGES];
static USABLE_END: [AtomicU64; MAX_USABLE_RANGES] = [ZERO; MAX_USABLE_RANGES];
static USABLE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Tell the allocator which physical ranges the firmware says are free.
///
/// Until this exists, the allocator assumed every byte from its base to one gigabyte
/// above it belonged to the kernel. Under QEMU that is nearly true. On a real machine
/// it is not: the firmware keeps regions of low memory for itself, and the ranges it
/// marks reserved are reserved because something is in them. Writing there corrupts
/// whatever the firmware is doing, with no fault and no message -- the machine simply
/// starts behaving oddly somewhere else entirely.
///
/// Ranges are half-open, `[start, end)`, and need not be sorted. Anything outside every
/// range is marked in use before the first allocation, so it can never be handed out.
///
/// Called before the allocator is used. With no call at all the behaviour is what it
/// has always been -- the whole window is assumed free -- so the `-kernel` boot path and
/// the host tests are unaffected.
pub fn declare_usable(ranges: &[(u64, u64)]) {
    let n = ranges.len().min(MAX_USABLE_RANGES);
    for (i, &(start, end)) in ranges.iter().take(n).enumerate() {
        USABLE_START[i].store(start, Ordering::SeqCst);
        USABLE_END[i].store(end, Ordering::SeqCst);
    }
    USABLE_COUNT.store(n as u64, Ordering::SeqCst);
}

/// Is this physical address inside a range the firmware called usable?
///
/// With no ranges declared every address counts as usable, which is the old behaviour
/// and what the host tests and the QEMU `-kernel` path rely on.
fn is_usable(phys: u64) -> bool {
    let n = USABLE_COUNT.load(Ordering::Relaxed) as usize;
    if n == 0 {
        return true;
    }
    for i in 0..n {
        let start = USABLE_START[i].load(Ordering::Relaxed);
        let end = USABLE_END[i].load(Ordering::Relaxed);
        if phys >= start && phys + FRAME_SIZE <= end {
            return true;
        }
    }
    false
}

/// Mark every frame that is not wholly inside a usable range as already in use.
///
/// A frame straddling the edge of a usable range counts as unusable: half a frame of
/// firmware memory is as bad as a whole one.
fn reserve_unusable() {
    if USABLE_COUNT.load(Ordering::Relaxed) == 0 {
        RESERVED_FRAMES.store(0, Ordering::SeqCst);
        return;
    }
    let base = BASE_PHYS.load(Ordering::Relaxed);
    let mut reserved = 0u64;
    for idx in 0..TOTAL_FRAMES {
        if !is_usable(base + idx as u64 * FRAME_SIZE) {
            let word = &BITMAP[idx / 64];
            word.fetch_or(1u64 << (idx % 64), Ordering::SeqCst);
            reserved += 1;
        }
    }
    RESERVED_FRAMES.store(reserved, Ordering::SeqCst);
}

/// How many frames the allocator may actually hand out, after reservations.
pub fn usable_frames() -> u64 {
    let mut n = 0;
    for word in BITMAP.iter() {
        n += word.load(Ordering::Relaxed).count_zeros() as u64;
    }
    n
}

/// Reset the allocator and configure the physical base address that frame 0
/// maps to. Calling `init` clears all bookkeeping, then re-applies whatever
/// [`declare_usable`] was told, so a reset cannot quietly hand back the
/// firmware's memory.
pub fn init(base_phys: u64) {
    BASE_PHYS.store(base_phys, Ordering::SeqCst);
    for word in BITMAP.iter() {
        word.store(0, Ordering::SeqCst);
    }
    reserve_unusable();
}

/// Allocate a single 4 KiB frame; returns the physical address or `None` when
/// the pool is exhausted.
pub fn alloc_frame() -> Option<u64> {
    for (idx, word) in BITMAP.iter().enumerate() {
        loop {
            let cur = word.load(Ordering::Relaxed);
            if cur == u64::MAX {
                break;
            }
            let inverse = !cur;
            let bit = inverse.trailing_zeros() as u64;
            let mask = 1u64 << bit;
            match word.compare_exchange_weak(
                cur,
                cur | mask,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let frame_idx = (idx as u64) * 64 + bit;
                    let base = BASE_PHYS.load(Ordering::Relaxed);
                    return Some(base + frame_idx * FRAME_SIZE);
                }
                Err(_) => continue,
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    OutOfRange,
    Misaligned,
}

/// Free a previously allocated frame. Idempotent on already-free frames is
/// not supported: the caller must not free the same frame twice.
pub fn free_frame(phys: u64) -> Result<(), FrameError> {
    let base = BASE_PHYS.load(Ordering::Relaxed);
    if phys < base {
        return Err(FrameError::OutOfRange);
    }
    let offset = phys - base;
    if !offset.is_multiple_of(FRAME_SIZE) {
        return Err(FrameError::Misaligned);
    }
    let frame_idx = offset / FRAME_SIZE;
    if frame_idx as usize >= TOTAL_FRAMES {
        return Err(FrameError::OutOfRange);
    }
    let word_idx = (frame_idx / 64) as usize;
    let bit = frame_idx % 64;
    let mask = 1u64 << bit;
    BITMAP[word_idx].fetch_and(!mask, Ordering::SeqCst);
    Ok(())
}

/// Count of frames currently marked as in-use.
/// How many frames the allocator may ever hand out on this machine.
///
/// `TOTAL_FRAMES` is the size of the bitmap -- a fact about this source file, not about
/// the machine. What matters is the bitmap minus the frames reserved because the
/// firmware said they are not ours.
///
/// The first version of this added free to used, which is every bit in the bitmap and
/// therefore the constant it was supposed to replace. The honest boot marker caught it
/// on the first run: it printed 262144 on a 256 MiB machine, which is four times the
/// memory present, and that is how a number that cannot be wrong earns its keep.
pub fn capacity_frames() -> u64 {
    TOTAL_FRAMES as u64 - RESERVED_FRAMES.load(Ordering::Relaxed)
}

/// Frames marked in use by [`reserve_unusable`] rather than by an allocation.
static RESERVED_FRAMES: AtomicU64 = AtomicU64::new(0);

pub fn used_frames() -> u64 {
    let mut total = 0u64;
    for word in BITMAP.iter() {
        total += word.load(Ordering::Relaxed).count_ones() as u64;
    }
    total
}

/// Outcome of `self_check` so the caller can emit precise diagnostic markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfCheckOk {
    pub burst_alloc: u32,
    pub burst_free: u32,
    pub stress_iterations: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    BurstAllocFailed { at: u32 },
    BurstFreeFailed { at: u32 },
    LeakAfterBurst { used: u64 },
    StressAllocFailed { at: u32 },
    StressFreeFailed { at: u32 },
    LeakAfterStress { used: u64 },
}

/// Default physical base for the bitmap, chosen to live above the kernel
/// image. The kernel ELF lands at `0x10_0000` (PVH default) and the
/// largest realistic kernel image we ship (~1 MiB) fits well below
/// `0x20_0000`, so frames carved out from `BASE_PHYS_DEFAULT` upward do
/// not alias the kernel `.text` / `.data` sections.
pub const BASE_PHYS_DEFAULT: u64 = 0x20_0000;

/// Acceptance test:
/// - allocate 1000 frames, then free all 1000, expect zero leak.
/// - run 1_000_000 single-frame alloc+free iterations, expect zero leak.
pub fn self_check() -> Result<SelfCheckOk, SelfCheckErr> {
    init(BASE_PHYS_DEFAULT);

    let mut frames = [0u64; 1000];
    for (i, slot) in frames.iter_mut().enumerate() {
        match alloc_frame() {
            Some(p) => *slot = p,
            None => return Err(SelfCheckErr::BurstAllocFailed { at: i as u32 }),
        }
    }
    if used_frames() != 1000 {
        return Err(SelfCheckErr::LeakAfterBurst { used: used_frames() });
    }
    for (i, slot) in frames.iter().enumerate() {
        if free_frame(*slot).is_err() {
            return Err(SelfCheckErr::BurstFreeFailed { at: i as u32 });
        }
    }
    if used_frames() != 0 {
        return Err(SelfCheckErr::LeakAfterBurst { used: used_frames() });
    }

    let stress_iterations: u32 = 1_000_000;
    for i in 0..stress_iterations {
        let p = match alloc_frame() {
            Some(v) => v,
            None => return Err(SelfCheckErr::StressAllocFailed { at: i }),
        };
        if free_frame(p).is_err() {
            return Err(SelfCheckErr::StressFreeFailed { at: i });
        }
    }
    if used_frames() != 0 {
        return Err(SelfCheckErr::LeakAfterStress { used: used_frames() });
    }

    Ok(SelfCheckOk {
        burst_alloc: 1000,
        burst_free: 1000,
        stress_iterations,
    })
}

#[cfg(test)]
mod memory_map_tests {
    use super::*;

    /// One test, not three, and deliberately.
    ///
    /// The allocator is global state -- one bitmap, one set of declared ranges -- and
    /// `cargo test` runs test functions on several threads at once. Three separate
    /// tests trod on each other and failed by one frame here and a quarter of a
    /// million there, which looks exactly like a real defect and is not one. Keeping
    /// the sequence in a single function is the honest fix; a lock would only hide
    /// that the state is shared.
    #[test]
    fn the_allocator_hands_out_only_what_the_firmware_called_usable() {
        // --- 1. With no map, the whole window is on offer. -------------------------
        //
        // This is the defect, written down. The old allocator assumed a gigabyte from
        // its base was free. On a machine with 256 MiB that is four times the RAM
        // present; on a real machine the window covers regions the firmware is using,
        // and writing there corrupts them with no fault and no message.
        declare_usable(&[]);
        init(BASE_PHYS_DEFAULT);
        assert_eq!(
            usable_frames(),
            TOTAL_FRAMES as u64,
            "with no map declared the whole window stays on offer, which is what the              QEMU -kernel path and the host tests depend on"
        );
        let beyond_a_small_machine = BASE_PHYS_DEFAULT + 256 * 1024 * 1024;
        assert!(
            is_usable(beyond_a_small_machine),
            "and it has no way of knowing that address is not memory at all"
        );

        // --- 2. With a map, everything outside it is out of reach. -----------------
        let usable_bytes = 16 * 1024 * 1024u64;
        declare_usable(&[(BASE_PHYS_DEFAULT, BASE_PHYS_DEFAULT + usable_bytes)]);
        init(BASE_PHYS_DEFAULT);
        assert_eq!(
            usable_frames(),
            usable_bytes / FRAME_SIZE,
            "exactly the declared region is free, and nothing else"
        );
        // And the number the boot log prints has to agree. The first version of
        // `capacity_frames` added free to used, which is every bit in the bitmap, so it
        // reported the whole window no matter what the map said.
        assert_eq!(
            capacity_frames(),
            usable_bytes / FRAME_SIZE,
            "capacity must be what the machine has, not the size of the bitmap"
        );
        assert!(
            capacity_frames() < TOTAL_FRAMES as u64,
            "a restricted map must report fewer frames than the bitmap holds"
        );

        let mut handed = 0u64;
        while let Some(phys) = alloc_frame() {
            assert!(
                phys >= BASE_PHYS_DEFAULT && phys < BASE_PHYS_DEFAULT + usable_bytes,
                "handed out {phys:#x}, outside the usable region"
            );
            handed += 1;
            assert!(handed <= usable_bytes / FRAME_SIZE, "handed out more than exists");
        }
        assert_eq!(handed, usable_bytes / FRAME_SIZE, "the whole region should be usable");
        assert!(alloc_frame().is_none(), "past the end it must refuse, not wander on");

        // --- 3. A hole in the middle is respected, not rounded over. ---------------
        let mib = 1024 * 1024u64;
        let lo = (BASE_PHYS_DEFAULT, BASE_PHYS_DEFAULT + 4 * mib);
        let hole_end = BASE_PHYS_DEFAULT + 8 * mib;
        let hi = (hole_end, hole_end + 4 * mib);
        declare_usable(&[lo, hi]);
        init(BASE_PHYS_DEFAULT);
        assert_eq!(usable_frames(), 8 * mib / FRAME_SIZE, "two regions of four MiB");
        while let Some(phys) = alloc_frame() {
            let inside = (phys >= lo.0 && phys < lo.1) || (phys >= hi.0 && phys < hi.1);
            assert!(inside, "handed out {phys:#x}, inside the reserved hole");
        }

        // Leave the global state as the other tests expect to find it.
        declare_usable(&[]);
        init(BASE_PHYS_DEFAULT);
    }
}
