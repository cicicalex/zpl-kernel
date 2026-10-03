#![no_std]

// Strict Multiboot2 wiring template.
// Intended to be copied into an external boot stage project.

use zpl_kernel::{
    enter_from_external_runtime, BootStackKind, ExternalBootFrameV1, ExternalBootRuntime,
    Multiboot2HandoffV1, PublishedHandoff, Stivale2HandoffV1,
};
use zpl_external_loader_runtime::bindings::{
    Multiboot2RuntimeInfoV1, RUNTIME_INFO_V1_MAGIC, RUNTIME_INFO_V1_VERSION, RUNTIME_MAX_CPU_COUNT,
    RUNTIME_MAX_MEMORY_MAP_ENTRIES,
};
use zpl_external_loader_runtime::{zpl_external_handoff_multiboot2_v1, HANDOFF_MODE_MULTIBOOT2};

pub struct Multiboot2Runtime;

fn halt_strict_mode_violation() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

impl ExternalBootRuntime for Multiboot2Runtime {
    fn publish_native_frame(&mut self, _frame: ExternalBootFrameV1) -> PublishedHandoff {
        halt_strict_mode_violation()
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

    fn publish_stivale2_handoff(&mut self, _handoff: Stivale2HandoffV1) -> PublishedHandoff {
        halt_strict_mode_violation()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Multiboot2RuntimeInput {
    pub total_memory_kib: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

fn validate_input(input: Multiboot2RuntimeInput) {
    if input.total_memory_kib == 0
        || input.cpu_count == 0
        || input.cpu_count > RUNTIME_MAX_CPU_COUNT
        || input.memory_map_entries == 0
        || input.memory_map_entries > RUNTIME_MAX_MEMORY_MAP_ENTRIES
    {
        halt_strict_mode_violation();
    }
}

pub unsafe fn boot_multiboot2_strict(input: Multiboot2RuntimeInput) -> ! {
    validate_input(input);
    let mut runtime = Multiboot2Runtime;

    let lower_memory_kib = 640u32;
    let upper_memory_kib = input
        .total_memory_kib
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
        total_memory_kib: input.total_memory_kib,
        cpu_count: input.cpu_count.max(1),
        memory_map_entries: input.memory_map_entries,
    };
    let stivale2 = Stivale2HandoffV1 {
        memory_bytes: input.total_memory_kib.saturating_mul(1024),
        cpu_count: input.cpu_count.max(1),
        memory_map_entries: input.memory_map_entries,
    };

    enter_from_external_runtime(
        &mut runtime,
        BootStackKind::Multiboot2,
        native,
        multiboot2,
        stivale2,
    )
}

/// Runtime-info v1 strict path.
/// Prefer this when you already have versioned metadata and want fail-closed C ABI entrypoint.
pub unsafe fn boot_multiboot2_strict_runtime_v1(input: Multiboot2RuntimeInput) -> ! {
    validate_input(input);

    let info = Multiboot2RuntimeInfoV1 {
        protocol_magic: RUNTIME_INFO_V1_MAGIC,
        protocol_version: RUNTIME_INFO_V1_VERSION,
        total_memory_bytes: input.total_memory_kib.saturating_mul(1024),
        cpu_count: input.cpu_count,
        memory_map_entries: input.memory_map_entries,
    };

    zpl_external_handoff_multiboot2_v1(core::ptr::addr_of!(info), HANDOFF_MODE_MULTIBOOT2)
}
