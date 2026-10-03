#![no_std]

pub mod bindings;

use zpl_kernel::{
    enter_from_external_runtime, BootStackKind, ExternalBootFrameV1, ExternalBootRuntime,
    Multiboot2HandoffV1, PublishedHandoff, Stivale2HandoffV1,
};

pub const HANDOFF_MODE_NATIVE_V1: u32 = 0;
pub const HANDOFF_MODE_MULTIBOOT2: u32 = 1;
pub const HANDOFF_MODE_STIVALE2: u32 = 2;

#[derive(Debug, Clone, Copy)]
pub struct RuntimeSnapshot {
    pub memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

impl RuntimeSnapshot {
    pub fn is_valid(self) -> bool {
        self.memory_bytes > 0
            && self.cpu_count > 0
            && self.cpu_count <= bindings::RUNTIME_MAX_CPU_COUNT
            && self.memory_map_entries > 0
            && self.memory_map_entries <= bindings::RUNTIME_MAX_MEMORY_MAP_ENTRIES
    }

    pub fn as_native_frame(self) -> ExternalBootFrameV1 {
        let total_memory_kib = self.memory_bytes / 1024;
        let lower_memory_kib = 640u32;
        let upper_memory_kib = total_memory_kib
            .saturating_sub(lower_memory_kib as u64)
            .min(u32::MAX as u64) as u32;

        ExternalBootFrameV1 {
            magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
            lower_memory_kib,
            upper_memory_kib,
            cpu_count: self.cpu_count.max(1),
            memory_map_entries: self.memory_map_entries,
        }
    }

    pub fn as_multiboot2(self) -> Multiboot2HandoffV1 {
        Multiboot2HandoffV1 {
            total_memory_kib: self.memory_bytes / 1024,
            cpu_count: self.cpu_count.max(1),
            memory_map_entries: self.memory_map_entries,
        }
    }

    pub fn as_stivale2(self) -> Stivale2HandoffV1 {
        Stivale2HandoffV1 {
            memory_bytes: self.memory_bytes,
            cpu_count: self.cpu_count.max(1),
            memory_map_entries: self.memory_map_entries,
        }
    }
}

pub struct ExternalStageRuntime;

impl ExternalBootRuntime for ExternalStageRuntime {
    fn publish_native_frame(&mut self, frame: ExternalBootFrameV1) -> PublishedHandoff {
        static mut SLOT: ExternalBootFrameV1 = ExternalBootFrameV1 {
            magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
            lower_memory_kib: 640,
            upper_memory_kib: 64 * 1024,
            cpu_count: 1,
            memory_map_entries: 1,
        };
        unsafe {
            SLOT = frame;
            PublishedHandoff {
                kind: BootStackKind::NativeV1,
                ptr: core::ptr::addr_of!(SLOT) as *const (),
            }
        }
    }

    fn publish_multiboot2_handoff(&mut self, handoff: Multiboot2HandoffV1) -> PublishedHandoff {
        static mut SLOT: Multiboot2HandoffV1 = Multiboot2HandoffV1 {
            total_memory_kib: 64 * 1024,
            cpu_count: 1,
            memory_map_entries: 1,
        };
        unsafe {
            SLOT = handoff;
            PublishedHandoff {
                kind: BootStackKind::Multiboot2,
                ptr: core::ptr::addr_of!(SLOT) as *const (),
            }
        }
    }

    fn publish_stivale2_handoff(&mut self, handoff: Stivale2HandoffV1) -> PublishedHandoff {
        static mut SLOT: Stivale2HandoffV1 = Stivale2HandoffV1 {
            memory_bytes: 64 * 1024 * 1024,
            cpu_count: 1,
            memory_map_entries: 1,
        };
        unsafe {
            SLOT = handoff;
            PublishedHandoff {
                kind: BootStackKind::Stivale2,
                ptr: core::ptr::addr_of!(SLOT) as *const (),
            }
        }
    }
}

/// Integrates real runtime snapshot into kernel handoff.
///
/// # Safety
/// Caller must ensure this function is invoked from boot stage context.
pub unsafe fn handoff_from_snapshot(snapshot: RuntimeSnapshot, kind: BootStackKind) -> ! {
    if !snapshot.is_valid() {
        halt_invalid_input();
    }
    let mut runtime = ExternalStageRuntime;
    enter_from_external_runtime(
        &mut runtime,
        kind,
        snapshot.as_native_frame(),
        snapshot.as_multiboot2(),
        snapshot.as_stivale2(),
    )
}

fn halt_invalid_input() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn parse_kind(mode: u32) -> BootStackKind {
    match mode {
        HANDOFF_MODE_NATIVE_V1 => BootStackKind::NativeV1,
        HANDOFF_MODE_MULTIBOOT2 => BootStackKind::Multiboot2,
        HANDOFF_MODE_STIVALE2 => BootStackKind::Stivale2,
        _ => halt_invalid_input(),
    }
}

pub fn handoff_from_memory_map_summary(
    summary: bindings::RuntimeMemoryMapSummary,
    kind: BootStackKind,
) -> ! {
    let snapshot = RuntimeSnapshot {
        memory_bytes: summary.total_memory_bytes,
        cpu_count: summary.cpu_count,
        memory_map_entries: summary.memory_map_entries,
    };
    if !snapshot.is_valid() {
        halt_invalid_input();
    }
    // SAFETY: this helper is for validated runtime summaries and transfers control immediately.
    unsafe { handoff_from_snapshot(snapshot, kind) }
}

/// C ABI entrypoint for direct runtime memory-map handoff.
///
/// `mode`: `HANDOFF_MODE_NATIVE_V1`, `HANDOFF_MODE_MULTIBOOT2`, or `HANDOFF_MODE_STIVALE2`.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_memory_map_v1(
    ptr: *const bindings::RuntimeMemoryMapV1,
    mode: u32,
) -> ! {
    let summary = match unsafe { bindings::summary_from_memory_map_ptr(ptr) } {
        Some(v) => v,
        None => halt_invalid_input(),
    };
    handoff_from_memory_map_summary(summary, parse_kind(mode))
}

/// C ABI strict entrypoint for memory-map -> native-v1 handoff.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_memory_map_native_v1(
    ptr: *const bindings::RuntimeMemoryMapV1,
) -> ! {
    zpl_external_handoff_memory_map_v1(ptr, HANDOFF_MODE_NATIVE_V1)
}

/// C ABI strict entrypoint for memory-map -> multiboot2 handoff.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_memory_map_multiboot2_v1(
    ptr: *const bindings::RuntimeMemoryMapV1,
) -> ! {
    zpl_external_handoff_memory_map_v1(ptr, HANDOFF_MODE_MULTIBOOT2)
}

/// C ABI strict entrypoint for memory-map -> stivale2 handoff.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_memory_map_stivale2_v1(
    ptr: *const bindings::RuntimeMemoryMapV1,
) -> ! {
    zpl_external_handoff_memory_map_v1(ptr, HANDOFF_MODE_STIVALE2)
}

/// C ABI entrypoint for native-v1 runtime metadata handoff.
///
/// `mode` must be `HANDOFF_MODE_NATIVE_V1`.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_native_v1(
    ptr: *const bindings::NativeRuntimeInfoV1,
    mode: u32,
) -> ! {
    if mode != HANDOFF_MODE_NATIVE_V1 {
        halt_invalid_input();
    }
    let snapshot = match unsafe { bindings::snapshot_from_native_ptr(ptr) } {
        Some(v) => v,
        None => halt_invalid_input(),
    };
    handoff_from_snapshot(snapshot, parse_kind(mode))
}

/// C ABI entrypoint for multiboot2-style runtime metadata handoff.
///
/// `mode`: `HANDOFF_MODE_NATIVE_V1`, `HANDOFF_MODE_MULTIBOOT2`, or `HANDOFF_MODE_STIVALE2`.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_multiboot2_v1(
    ptr: *const bindings::Multiboot2RuntimeInfoV1,
    mode: u32,
) -> ! {
    let snapshot = match unsafe { bindings::snapshot_from_multiboot2_ptr(ptr) } {
        Some(v) => v,
        None => halt_invalid_input(),
    };
    handoff_from_snapshot(snapshot, parse_kind(mode))
}

/// C ABI entrypoint for stivale2-style runtime metadata handoff.
///
/// `mode`: `HANDOFF_MODE_NATIVE_V1`, `HANDOFF_MODE_MULTIBOOT2`, or `HANDOFF_MODE_STIVALE2`.
#[no_mangle]
pub unsafe extern "C" fn zpl_external_handoff_stivale2_v1(
    ptr: *const bindings::Stivale2RuntimeInfoV1,
    mode: u32,
) -> ! {
    let snapshot = match unsafe { bindings::snapshot_from_stivale2_ptr(ptr) } {
        Some(v) => v,
        None => halt_invalid_input(),
    };
    handoff_from_snapshot(snapshot, parse_kind(mode))
}
