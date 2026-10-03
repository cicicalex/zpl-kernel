//! The shapes a bootloader hands the kernel, and the kind of boot each one means.
//!
//! Three entry paths exist -- a plain handoff, Multiboot2 and stivale2 -- and
//! each arrives with its own structure. They are converted to one internal
//! `BootInfo` early, so nothing past the entry point has to care which
//! bootloader started the machine.
use crate::arch::x86_64::ExternalBootFrameV1;
use crate::boot::BootInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStackKind {
    NativeV1,
    Multiboot2,
    Stivale2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Multiboot2HandoffV1 {
    pub total_memory_kib: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Stivale2HandoffV1 {
    pub memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

impl Multiboot2HandoffV1 {
    pub fn to_external_frame(self) -> ExternalBootFrameV1 {
        let lower_memory_kib = 640u32;
        let upper = self.total_memory_kib.saturating_sub(lower_memory_kib as u64);
        let upper_memory_kib = upper.min(u32::MAX as u64) as u32;

        ExternalBootFrameV1 {
            magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
            lower_memory_kib,
            upper_memory_kib,
            cpu_count: self.cpu_count.max(1),
            memory_map_entries: self.memory_map_entries,
        }
    }
}

impl Stivale2HandoffV1 {
    pub fn to_external_frame(self) -> ExternalBootFrameV1 {
        let total_memory_kib = self.memory_bytes / 1024;
        let lower_memory_kib = 640u32;
        let upper = total_memory_kib.saturating_sub(lower_memory_kib as u64);
        let upper_memory_kib = upper.min(u32::MAX as u64) as u32;

        ExternalBootFrameV1 {
            magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
            lower_memory_kib,
            upper_memory_kib,
            cpu_count: self.cpu_count.max(1),
            memory_map_entries: self.memory_map_entries,
        }
    }
}

/// # Safety
/// `ptr` must be valid for reading one `Multiboot2HandoffV1`.
pub unsafe fn boot_info_from_multiboot2_ptr(ptr: *const Multiboot2HandoffV1) -> Option<BootInfo> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a valid pointer for one read.
    let handoff = unsafe { core::ptr::read(ptr) };
    crate::arch::x86_64::parse_external_boot_frame(handoff.to_external_frame())
}

/// # Safety
/// `ptr` must be valid for reading one `Stivale2HandoffV1`.
pub unsafe fn boot_info_from_stivale2_ptr(ptr: *const Stivale2HandoffV1) -> Option<BootInfo> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees a valid pointer for one read.
    let handoff = unsafe { core::ptr::read(ptr) };
    crate::arch::x86_64::parse_external_boot_frame(handoff.to_external_frame())
}
