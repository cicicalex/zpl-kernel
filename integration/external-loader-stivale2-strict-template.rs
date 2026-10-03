#![no_std]

// Strict Stivale2 wiring template.
// Intended to be copied into an external boot stage project.

use zpl_kernel::{
    enter_from_external_runtime, BootStackKind, ExternalBootFrameV1, ExternalBootRuntime,
    Multiboot2HandoffV1, PublishedHandoff, Stivale2HandoffV1,
};
use zpl_external_loader_runtime::bindings::{
    RUNTIME_INFO_V1_MAGIC, RUNTIME_INFO_V1_VERSION, RUNTIME_MAX_CPU_COUNT,
    RUNTIME_MAX_MEMORY_MAP_ENTRIES, Stivale2RuntimeInfoV1,
};
use zpl_external_loader_runtime::{zpl_external_handoff_stivale2_v1, HANDOFF_MODE_STIVALE2};

pub struct Stivale2Runtime;

fn halt_strict_mode_violation() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

impl ExternalBootRuntime for Stivale2Runtime {
    fn publish_native_frame(&mut self, _frame: ExternalBootFrameV1) -> PublishedHandoff {
        halt_strict_mode_violation()
    }

    fn publish_multiboot2_handoff(&mut self, _handoff: Multiboot2HandoffV1) -> PublishedHandoff {
        halt_strict_mode_violation()
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
pub struct Stivale2RuntimeInput {
    pub memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

fn validate_input(input: Stivale2RuntimeInput) {
    if input.memory_bytes == 0
        || input.cpu_count == 0
        || input.cpu_count > RUNTIME_MAX_CPU_COUNT
        || input.memory_map_entries == 0
        || input.memory_map_entries > RUNTIME_MAX_MEMORY_MAP_ENTRIES
    {
        halt_strict_mode_violation();
    }
}

pub unsafe fn boot_stivale2_strict(input: Stivale2RuntimeInput) -> ! {
    validate_input(input);
    let mut runtime = Stivale2Runtime;

    let total_memory_kib = input.memory_bytes / 1024;
    let lower_memory_kib = 640u32;
    let upper_memory_kib = total_memory_kib
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

    enter_from_external_runtime(
        &mut runtime,
        BootStackKind::Stivale2,
        native,
        multiboot2,
        stivale2,
    )
}

/// Runtime-info v1 strict path.
/// Prefer this when you already publish versioned metadata from the stage.
pub unsafe fn boot_stivale2_strict_runtime_v1(input: Stivale2RuntimeInput) -> ! {
    validate_input(input);

    let info = Stivale2RuntimeInfoV1 {
        protocol_magic: RUNTIME_INFO_V1_MAGIC,
        protocol_version: RUNTIME_INFO_V1_VERSION,
        total_memory_bytes: input.memory_bytes,
        cpu_count: input.cpu_count,
        memory_map_entries: input.memory_map_entries,
    };

    zpl_external_handoff_stivale2_v1(core::ptr::addr_of!(info), HANDOFF_MODE_STIVALE2)
}
