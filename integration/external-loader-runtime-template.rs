#![no_std]

// Template module intended for an external bootloader/stage project.
// Copy this file into the boot stage repo and wire real memory-map data.

use zpl_kernel::{
    enter_from_external_runtime, publish_external_boot_frame_v1, BootStackKind, ExternalBootFrameV1,
    ExternalBootRuntime, Multiboot2HandoffV1, PublishedHandoff, Stivale2HandoffV1,
};
use zpl_external_loader_runtime::bindings::{
    RUNTIME_MAX_CPU_COUNT, RUNTIME_MAX_MEMORY_MAP_ENTRIES,
};

pub struct ExternalStageRuntime;

impl ExternalBootRuntime for ExternalStageRuntime {
    fn publish_native_frame(&mut self, frame: ExternalBootFrameV1) -> PublishedHandoff {
        PublishedHandoff {
            kind: BootStackKind::NativeV1,
            ptr: publish_external_boot_frame_v1(&frame) as *const (),
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

#[derive(Debug, Clone, Copy)]
pub struct RuntimeInput {
    pub memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

const BOOTSTRAP_LOWER_MEMORY_KIB: u32 = 640;

fn halt_invalid_runtime_input() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn validate_runtime_input(input: RuntimeInput) {
    if input.memory_bytes == 0
        || input.cpu_count == 0
        || input.cpu_count > RUNTIME_MAX_CPU_COUNT
        || input.memory_map_entries == 0
        || input.memory_map_entries > RUNTIME_MAX_MEMORY_MAP_ENTRIES
    {
        halt_invalid_runtime_input();
    }
}

fn build_handoff_frames(
    input: RuntimeInput,
) -> (ExternalBootFrameV1, Multiboot2HandoffV1, Stivale2HandoffV1) {
    let lower_memory_kib = BOOTSTRAP_LOWER_MEMORY_KIB;
    let upper_memory_kib = (input.memory_bytes / 1024)
        .saturating_sub(lower_memory_kib as u64)
        .min(u32::MAX as u64) as u32;
    let native = ExternalBootFrameV1 {
        magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
        lower_memory_kib,
        upper_memory_kib,
        cpu_count: input.cpu_count.max(1),
        memory_map_entries: input.memory_map_entries,
    };
    let multiboot2 = Multiboot2HandoffV1 {
        total_memory_kib: input.memory_bytes / 1024,
        cpu_count: input.cpu_count.max(1),
        memory_map_entries: input.memory_map_entries,
    };
    let stivale2 = Stivale2HandoffV1 {
        memory_bytes: input.memory_bytes,
        cpu_count: input.cpu_count.max(1),
        memory_map_entries: input.memory_map_entries,
    };
    (native, multiboot2, stivale2)
}

fn boot_with_kind(input: RuntimeInput, kind: BootStackKind) -> ! {
    validate_runtime_input(input);
    let mut runtime = ExternalStageRuntime;
    let (native, multiboot2, stivale2) = build_handoff_frames(input);

    enter_from_external_runtime(
        &mut runtime,
        kind,
        native,
        multiboot2,
        stivale2,
    )
}

pub unsafe fn boot_from_multiboot2(input: RuntimeInput) -> ! {
    boot_with_kind(input, BootStackKind::Multiboot2)
}

pub unsafe fn boot_from_stivale2(input: RuntimeInput) -> ! {
    boot_with_kind(input, BootStackKind::Stivale2)
}

pub unsafe fn boot_from_native_v1(input: RuntimeInput) -> ! {
    boot_with_kind(input, BootStackKind::NativeV1)
}
