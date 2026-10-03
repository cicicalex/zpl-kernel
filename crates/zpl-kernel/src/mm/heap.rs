//! Bump-style global heap for the bare-metal kernel.
//!
//! Goals:
//! - Provide a `#[global_allocator]` so `alloc::boxed::Box`,
//!   `alloc::vec::Vec`, etc. are usable from the kernel.
//! - No external dependency. `linked_list_allocator` was a candidate,
//!   but a hand-rolled bump allocator avoids the dependency entirely
//!   while still covering the acceptance
//!   (boot self-check exercising `Box::new` + `Vec::new` + 100
//!   pushes).
//!
//! Tradeoffs:
//! - `dealloc` is a no-op (allocations leak). The kernel boot path
//!   only allocates a few hundred bytes today, so leaks are cheap.
//! - Heap region is 256 KiB carved out of a static `.bss` array. If
//!   future syscalls need more, swap this implementation for the
//!   `linked_list_allocator` once `#2` is officially approved.
//! - Concurrency: a single `AtomicUsize` cursor protects the bump
//!   pointer. Safe for SMP one-shot bring-up; production-grade
//!   allocators land alongside `#36 SMP`.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

pub const HEAP_SIZE: usize = 256 * 1024;

#[repr(C, align(16))]
struct HeapStorage([u8; HEAP_SIZE]);

static mut HEAP: HeapStorage = HeapStorage([0; HEAP_SIZE]);
static OFFSET: AtomicUsize = AtomicUsize::new(0);

pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(1);
        let size = layout.size();
        let base = (&raw const HEAP) as *const u8 as usize;

        loop {
            let cur = OFFSET.load(Ordering::Relaxed);
            let abs = base + cur;
            // Round up to alignment.
            let pad = abs.next_multiple_of(align) - abs;
            let new_off = cur.checked_add(pad).and_then(|v| v.checked_add(size));
            let Some(new_off) = new_off else {
                return core::ptr::null_mut();
            };
            if new_off > HEAP_SIZE {
                return core::ptr::null_mut();
            }
            if OFFSET
                .compare_exchange(cur, new_off, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return (base + cur + pad) as *mut u8;
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // bump allocator: deallocation is a no-op for v0.1.
    }
}

#[global_allocator]
static GLOBAL: BumpAllocator = BumpAllocator;

/// Reset the bump cursor to zero. Call once during boot.
///
/// # Safety
/// Caller must guarantee no allocations are live across the reset.
pub unsafe fn init() {
    OFFSET.store(0, Ordering::SeqCst);
}

pub fn used() -> usize {
    OFFSET.load(Ordering::SeqCst)
}

pub fn capacity() -> usize {
    HEAP_SIZE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    BoxAllocFailed,
    BoxValueWrong,
    VecLenWrong { expected: usize, got: usize },
    VecValueWrong { idx: usize },
    OutOfMemory,
}

extern crate alloc;

/// Acceptance flow: boxed value + vector with 100
/// elements survive a round-trip.
pub fn self_check() -> Result<(), SelfCheckErr> {
    let boxed = alloc::boxed::Box::new(0xCAFE_BABE_u64);
    if *boxed != 0xCAFE_BABE_u64 {
        return Err(SelfCheckErr::BoxValueWrong);
    }
    drop(boxed);

    let mut v: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    for i in 0..100u32 {
        v.push(i.wrapping_mul(7) ^ 0xA5A5);
    }
    if v.len() != 100 {
        return Err(SelfCheckErr::VecLenWrong {
            expected: 100,
            got: v.len(),
        });
    }
    for (idx, val) in v.iter().enumerate() {
        let expected = (idx as u32).wrapping_mul(7) ^ 0xA5A5;
        if *val != expected {
            return Err(SelfCheckErr::VecValueWrong { idx });
        }
    }
    drop(v);

    Ok(())
}
