//! Stack guard / overflow detection demo.
//!
//! Builds a small synthetic stack in the 4 KiB user-region paging slot
//! (`paging::USER_REGION_BASE`). Two pages are mapped (the "stack body"),
//! one page below the stack body is left unmapped — that is the guard
//! page. Writing past the lower edge of the stack body lands on the
//! guard, which trips the #PF handler installed in `interrupts.rs` and
//! produces a clean diagnostic line on COM1 instead of a triple fault.
//!
//! This module is bare-metal only.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::sync::atomic::Ordering;

use crate::mm::frame_alloc::alloc_frame;
use crate::mm::paging;

/// Layout inside the 4 KiB-paged user region (`USER_REGION_BASE`):
/// - `0x40000000`: paging acceptance test page (self-check).
/// - `0x40004000`: guard page (intentionally unmapped).
/// - `0x40005000..0x40007000`: 8 KiB synthetic stack body (mapped R/W).
const GUARD_PAGE: u64 = paging::USER_REGION_BASE + 0x4000;
const STACK_BODY_BASE: u64 = paging::USER_REGION_BASE + 0x5000;
const STACK_BODY_PAGES: u64 = 2;

#[derive(Debug, Clone, Copy)]
pub enum StackGuardError {
    NoFrame,
    MapFailed,
}

/// Map the synthetic stack body and verify the guard page below it
/// remains unmapped. No write is performed yet.
pub unsafe fn install() -> Result<(), StackGuardError> {
    for i in 0..STACK_BODY_PAGES {
        let frame = alloc_frame().ok_or(StackGuardError::NoFrame)?;
        let virt = STACK_BODY_BASE + i * 4096;
        unsafe { paging::map_4k_page(virt, frame).map_err(|_| StackGuardError::MapFailed)? };
    }
    Ok(())
}

/// Confirm the stack body is reachable: write a canary at the bottom of
/// the body and read it back through a volatile pointer.
pub fn smoke_write() -> bool {
    let canary: u64 = 0xC0DE_FACE_5AFE_F00D;
    unsafe {
        core::ptr::write_volatile(STACK_BODY_BASE as *mut u64, canary);
        core::ptr::read_volatile(STACK_BODY_BASE as *const u64) == canary
    }
}

/// Trigger an overflow by writing one quad-word into the guard page.
/// The #PF handler should observe it, increment the page-fault counter,
/// and return cleanly so we can verify in user space.
pub fn trigger_overflow() {
    unsafe {
        core::ptr::write_volatile(GUARD_PAGE as *mut u64, 0xDEAD_BEEF_DEAD_BEEF);
    }
}

/// Read the running page-fault count as observed by the #PF handler.
pub fn page_fault_count() -> usize {
    paging::PAGE_FAULT_COUNT.load(Ordering::SeqCst)
}

pub fn guard_addr() -> u64 {
    GUARD_PAGE
}
