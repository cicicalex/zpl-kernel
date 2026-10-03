//! The step between a bootloader's handoff and the kernel's own view of memory.
//!
//! It takes whatever the boot path published, validates it, and makes it
//! available as one shape. Split out from `boot` because it is the part that is
//! worth testing on a host, where there is no bootloader to ask.
use crate::arch::x86_64::{publish_external_boot_frame_v1, ExternalBootFrameV1};
use crate::mm::boot_stack::{BootStackKind, Multiboot2HandoffV1, Stivale2HandoffV1};

#[derive(Debug, Clone, Copy)]
pub struct PublishedHandoff {
    pub kind: BootStackKind,
    pub ptr: *const (),
}

static mut MULTIBOOT2_SLOT: Multiboot2HandoffV1 = Multiboot2HandoffV1 {
    total_memory_kib: 64 * 1024,
    cpu_count: 1,
    memory_map_entries: 1,
};

static mut STIVALE2_SLOT: Stivale2HandoffV1 = Stivale2HandoffV1 {
    memory_bytes: 64 * 1024 * 1024,
    cpu_count: 1,
    memory_map_entries: 1,
};

pub struct LoaderStageV1;

/// Runtime contract for external boot stages.
/// Implemented outside kernel crate by real bootloader runtime module.
pub trait ExternalBootRuntime {
    fn publish_native_frame(&mut self, frame: ExternalBootFrameV1) -> PublishedHandoff;
    fn publish_multiboot2_handoff(&mut self, handoff: Multiboot2HandoffV1) -> PublishedHandoff;
    fn publish_stivale2_handoff(&mut self, handoff: Stivale2HandoffV1) -> PublishedHandoff;
}

impl LoaderStageV1 {
    pub const fn new() -> Self {
        Self
    }

    /// Publisher contract:
    /// 1) Loader writes exactly one handoff frame to a static slot.
    /// 2) Loader passes returned pointer to matching `zpl_boot_entry_*`.
    /// 3) Slot must remain valid until kernel entry consumes pointer.
    pub fn publish_native(self, frame: ExternalBootFrameV1) -> PublishedHandoff {
        let ptr = publish_external_boot_frame_v1(&frame) as *const ();
        PublishedHandoff {
            kind: BootStackKind::NativeV1,
            ptr,
        }
    }

    pub fn publish_multiboot2(self, handoff: Multiboot2HandoffV1) -> PublishedHandoff {
        unsafe {
            MULTIBOOT2_SLOT = handoff;
            PublishedHandoff {
                kind: BootStackKind::Multiboot2,
                ptr: core::ptr::addr_of!(MULTIBOOT2_SLOT) as *const (),
            }
        }
    }

    pub fn publish_stivale2(self, handoff: Stivale2HandoffV1) -> PublishedHandoff {
        unsafe {
            STIVALE2_SLOT = handoff;
            PublishedHandoff {
                kind: BootStackKind::Stivale2,
                ptr: core::ptr::addr_of!(STIVALE2_SLOT) as *const (),
            }
        }
    }

    /// Transfers control to kernel entrypoint based on published handoff type.
    ///
    /// # Safety
    /// Caller must guarantee `handoff.ptr` is the matching type for `handoff.kind`
    /// and remains valid until entrypoint reads it.
    pub unsafe fn handoff_and_enter(self, handoff: PublishedHandoff) -> ! {
        match handoff.kind {
            BootStackKind::NativeV1 => crate::start::zpl_boot_entry_v1(
                handoff.ptr as *const ExternalBootFrameV1,
            ),
            BootStackKind::Multiboot2 => crate::start::zpl_boot_entry_multiboot2_v1(
                handoff.ptr as *const Multiboot2HandoffV1,
            ),
            BootStackKind::Stivale2 => crate::start::zpl_boot_entry_stivale2_v1(
                handoff.ptr as *const Stivale2HandoffV1,
            ),
        }
    }
}

impl Default for LoaderStageV1 {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper used by external bootloader runtime modules to keep handoff flow deterministic:
/// normalize handoff -> publish into stable slot -> jump to matching entrypoint.
///
/// # Safety
/// Caller must guarantee that all handoff payloads are initialized and that
/// `kind` matches the pointer contract expected by the selected entry path.
pub unsafe fn enter_from_external_runtime<R: ExternalBootRuntime>(
    runtime: &mut R,
    kind: BootStackKind,
    native: ExternalBootFrameV1,
    multiboot2: Multiboot2HandoffV1,
    stivale2: Stivale2HandoffV1,
) -> ! {
    let published = match kind {
        BootStackKind::NativeV1 => runtime.publish_native_frame(native),
        BootStackKind::Multiboot2 => runtime.publish_multiboot2_handoff(multiboot2),
        BootStackKind::Stivale2 => runtime.publish_stivale2_handoff(stivale2),
    };
    LoaderStageV1::new().handoff_and_enter(published)
}
