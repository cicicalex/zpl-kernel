#![no_std]

// Memory-map driven external boot stage template.
// Copy into your real boot-stage project and replace region discovery.

use zpl_external_loader_runtime::bindings::{
    summary_from_memory_map_ptr, RuntimeMemoryMapV1, RuntimeMemoryRegionV1,
    MEMORY_REGION_USABLE, RUNTIME_INFO_V1_MAGIC, RUNTIME_INFO_V1_VERSION, RUNTIME_MAX_CPU_COUNT,
    RUNTIME_MEMORY_MAP_MAX_REGIONS,
};
use zpl_external_loader_runtime::{
    zpl_external_handoff_memory_map_v1, HANDOFF_MODE_MULTIBOOT2, HANDOFF_MODE_NATIVE_V1,
    HANDOFF_MODE_STIVALE2,
};

fn halt_invalid_runtime_input() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn is_valid_handoff_mode(mode: u32) -> bool {
    mode == HANDOFF_MODE_NATIVE_V1 || mode == HANDOFF_MODE_MULTIBOOT2 || mode == HANDOFF_MODE_STIVALE2
}

pub struct CollectedMemoryMap<'a> {
    pub cpu_count: u32,
    pub regions: &'a [RuntimeMemoryRegionV1],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Multiboot2MmapEntryV1 {
    pub base_addr: u64,
    pub length: u64,
    pub entry_type: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Stivale2MmapEntryV1 {
    pub base: u64,
    pub length: u64,
    pub entry_type: u32,
    pub unused: u32,
}

pub trait BootStageMemoryMapSource {
    fn collect_memory_map(&self) -> Option<CollectedMemoryMap<'_>>;
}

pub struct StageSliceSource<'a> {
    pub cpu_count: u32,
    pub regions: &'a [RuntimeMemoryRegionV1],
}

impl BootStageMemoryMapSource for StageSliceSource<'_> {
    fn collect_memory_map(&self) -> Option<CollectedMemoryMap<'_>> {
        Some(CollectedMemoryMap {
            cpu_count: self.cpu_count,
            regions: self.regions,
        })
    }
}

fn is_valid_raw_region(base: u64, length: u64) -> bool {
    length > 0 && base.checked_add(length).is_some()
}

pub fn map_multiboot2_entries_to_runtime_regions<'a>(
    entries: &'a [Multiboot2MmapEntryV1],
    out: &'a mut [RuntimeMemoryRegionV1],
) -> Option<&'a [RuntimeMemoryRegionV1]> {
    if entries.is_empty() || out.is_empty() || entries.len() > out.len() {
        return None;
    }

    let mut written = 0usize;
    for entry in entries {
        if !is_valid_raw_region(entry.base_addr, entry.length) {
            return None;
        }
        let region_type = if entry.entry_type == 1 {
            MEMORY_REGION_USABLE
        } else {
            entry.entry_type
        };
        out[written] = RuntimeMemoryRegionV1 {
            base: entry.base_addr,
            length: entry.length,
            region_type,
            reserved: 0,
        };
        written += 1;
        if written >= out.len() {
            break;
        }
    }

    if written == 0 {
        return None;
    }
    Some(&out[..written])
}

pub fn map_stivale2_entries_to_runtime_regions<'a>(
    entries: &'a [Stivale2MmapEntryV1],
    out: &'a mut [RuntimeMemoryRegionV1],
) -> Option<&'a [RuntimeMemoryRegionV1]> {
    if entries.is_empty() || out.is_empty() || entries.len() > out.len() {
        return None;
    }

    let mut written = 0usize;
    for entry in entries {
        if !is_valid_raw_region(entry.base, entry.length) {
            return None;
        }
        let region_type = if entry.entry_type == 1 {
            MEMORY_REGION_USABLE
        } else {
            entry.entry_type
        };
        out[written] = RuntimeMemoryRegionV1 {
            base: entry.base,
            length: entry.length,
            region_type,
            reserved: 0,
        };
        written += 1;
        if written >= out.len() {
            break;
        }
    }

    if written == 0 {
        return None;
    }
    Some(&out[..written])
}

/// # Safety
/// `entries_ptr` must reference a valid contiguous array with `entry_count` elements.
pub unsafe fn boot_from_multiboot2_mmap_ptr(
    entries_ptr: *const Multiboot2MmapEntryV1,
    entry_count: u32,
    cpu_count: u32,
    mode: u32,
    out_regions: &mut [RuntimeMemoryRegionV1],
) -> ! {
    if entries_ptr.is_null()
        || entry_count == 0
        || cpu_count == 0
        || cpu_count > RUNTIME_MAX_CPU_COUNT
        || !is_valid_handoff_mode(mode)
    {
        halt_invalid_runtime_input();
    }
    if entry_count as usize > out_regions.len()
        || entry_count > RUNTIME_MEMORY_MAP_MAX_REGIONS
    {
        halt_invalid_runtime_input();
    }

    // SAFETY: caller guarantees pointer validity for `entry_count` records.
    let entries = unsafe { core::slice::from_raw_parts(entries_ptr, entry_count as usize) };
    let regions = match map_multiboot2_entries_to_runtime_regions(entries, out_regions) {
        Some(v) => v,
        None => halt_invalid_runtime_input(),
    };

    let source = StageSliceSource { cpu_count, regions };
    boot_from_memory_map_source(&source, mode)
}

/// # Safety
/// `entries_ptr` must reference a valid contiguous array with `entry_count` elements.
pub unsafe fn boot_from_multiboot2_mmap_ptr_strict(
    entries_ptr: *const Multiboot2MmapEntryV1,
    entry_count: u32,
    cpu_count: u32,
    out_regions: &mut [RuntimeMemoryRegionV1],
) -> ! {
    boot_from_multiboot2_mmap_ptr(
        entries_ptr,
        entry_count,
        cpu_count,
        HANDOFF_MODE_MULTIBOOT2,
        out_regions,
    )
}

/// # Safety
/// `entries_ptr` must reference a valid contiguous array with `entry_count` elements.
pub unsafe fn boot_from_stivale2_mmap_ptr(
    entries_ptr: *const Stivale2MmapEntryV1,
    entry_count: u32,
    cpu_count: u32,
    mode: u32,
    out_regions: &mut [RuntimeMemoryRegionV1],
) -> ! {
    if entries_ptr.is_null()
        || entry_count == 0
        || cpu_count == 0
        || cpu_count > RUNTIME_MAX_CPU_COUNT
        || !is_valid_handoff_mode(mode)
    {
        halt_invalid_runtime_input();
    }
    if entry_count as usize > out_regions.len()
        || entry_count > RUNTIME_MEMORY_MAP_MAX_REGIONS
    {
        halt_invalid_runtime_input();
    }

    // SAFETY: caller guarantees pointer validity for `entry_count` records.
    let entries = unsafe { core::slice::from_raw_parts(entries_ptr, entry_count as usize) };
    let regions = match map_stivale2_entries_to_runtime_regions(entries, out_regions) {
        Some(v) => v,
        None => halt_invalid_runtime_input(),
    };

    let source = StageSliceSource { cpu_count, regions };
    boot_from_memory_map_source(&source, mode)
}

/// # Safety
/// `entries_ptr` must reference a valid contiguous array with `entry_count` elements.
pub unsafe fn boot_from_stivale2_mmap_ptr_strict(
    entries_ptr: *const Stivale2MmapEntryV1,
    entry_count: u32,
    cpu_count: u32,
    out_regions: &mut [RuntimeMemoryRegionV1],
) -> ! {
    boot_from_stivale2_mmap_ptr(
        entries_ptr,
        entry_count,
        cpu_count,
        HANDOFF_MODE_STIVALE2,
        out_regions,
    )
}

/// # Safety
/// Call only from external stage context with valid control transfer semantics.
pub unsafe fn boot_from_memory_map_source<S: BootStageMemoryMapSource>(
    source: &S,
    mode: u32,
) -> ! {
    if !is_valid_handoff_mode(mode) {
        halt_invalid_runtime_input();
    }

    let collected = match source.collect_memory_map() {
        Some(v) => v,
        None => halt_invalid_runtime_input(),
    };
    if collected.cpu_count == 0
        || collected.cpu_count > RUNTIME_MAX_CPU_COUNT
        || collected.regions.is_empty()
    {
        halt_invalid_runtime_input();
    }
    if collected.regions.len() > RUNTIME_MEMORY_MAP_MAX_REGIONS as usize {
        halt_invalid_runtime_input();
    }

    let map = RuntimeMemoryMapV1 {
        protocol_magic: RUNTIME_INFO_V1_MAGIC,
        protocol_version: RUNTIME_INFO_V1_VERSION,
        cpu_count: collected.cpu_count,
        region_count: collected.regions.len() as u32,
        regions_ptr: collected.regions.as_ptr(),
    };

    // Pre-validate summary so runtime entrypoint receives sane map.
    if unsafe { summary_from_memory_map_ptr(core::ptr::addr_of!(map)) }.is_none() {
        halt_invalid_runtime_input();
    }

    zpl_external_handoff_memory_map_v1(core::ptr::addr_of!(map), mode)
}

/// # Safety
/// Call only from external stage context with valid control transfer semantics.
pub unsafe fn boot_from_memory_map(
    cpu_count: u32,
    regions: &[RuntimeMemoryRegionV1],
    mode: u32,
) -> ! {
    if cpu_count == 0
        || cpu_count > RUNTIME_MAX_CPU_COUNT
        || regions.is_empty()
        || !is_valid_handoff_mode(mode)
    {
        halt_invalid_runtime_input();
    }
    if regions.len() > RUNTIME_MEMORY_MAP_MAX_REGIONS as usize {
        halt_invalid_runtime_input();
    }

    let source = StageSliceSource { cpu_count, regions };
    boot_from_memory_map_source(&source, mode)
}

/// # Safety
/// Call only from external stage context with validated runtime memory map data.
pub unsafe fn boot_from_memory_map_multiboot2(
    cpu_count: u32,
    regions: &[RuntimeMemoryRegionV1],
) -> ! {
    boot_from_memory_map(cpu_count, regions, HANDOFF_MODE_MULTIBOOT2)
}

/// # Safety
/// Call only from external stage context with validated runtime memory map data.
pub unsafe fn boot_from_memory_map_stivale2(
    cpu_count: u32,
    regions: &[RuntimeMemoryRegionV1],
) -> ! {
    boot_from_memory_map(cpu_count, regions, HANDOFF_MODE_STIVALE2)
}

/// # Safety
/// Call only from external stage context with validated runtime memory map data.
pub unsafe fn boot_from_memory_map_native_v1(
    cpu_count: u32,
    regions: &[RuntimeMemoryRegionV1],
) -> ! {
    boot_from_memory_map(cpu_count, regions, HANDOFF_MODE_NATIVE_V1)
}
