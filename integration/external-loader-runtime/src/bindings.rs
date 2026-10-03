use crate::RuntimeSnapshot;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Multiboot2RuntimeInfoV1 {
    pub protocol_magic: u32,
    pub protocol_version: u32,
    pub total_memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Stivale2RuntimeInfoV1 {
    pub protocol_magic: u32,
    pub protocol_version: u32,
    pub total_memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NativeRuntimeInfoV1 {
    pub protocol_magic: u32,
    pub protocol_version: u32,
    pub total_memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

pub const RUNTIME_INFO_V1_MAGIC: u32 = 0x5A_50_4C_52; // "ZPLR"
pub const RUNTIME_INFO_V1_VERSION: u32 = 1;
pub const MEMORY_REGION_USABLE: u32 = 1;
pub const RUNTIME_MEMORY_MAP_MAX_REGIONS: u32 = 4096;
pub const RUNTIME_MAX_CPU_COUNT: u32 = 4096;
pub const RUNTIME_MAX_MEMORY_MAP_ENTRIES: u32 = RUNTIME_MEMORY_MAP_MAX_REGIONS;

#[derive(Debug, Clone, Copy)]
pub struct RuntimeMemoryMapSummary {
    pub total_memory_bytes: u64,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

impl RuntimeMemoryMapSummary {
    pub fn is_valid(self) -> bool {
        self.total_memory_bytes > 0
            && self.cpu_count > 0
            && self.cpu_count <= RUNTIME_MAX_CPU_COUNT
            && self.memory_map_entries > 0
            && self.memory_map_entries <= RUNTIME_MAX_MEMORY_MAP_ENTRIES
    }
}

pub trait RuntimeMemoryMapSource {
    fn total_memory_bytes(&self) -> u64;
    fn cpu_count(&self) -> u32;
    fn memory_map_entries(&self) -> u32;

    fn to_summary(&self) -> RuntimeMemoryMapSummary {
        RuntimeMemoryMapSummary {
            total_memory_bytes: self.total_memory_bytes(),
            cpu_count: self.cpu_count(),
            memory_map_entries: self.memory_map_entries(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RuntimeMemoryRegionV1 {
    pub base: u64,
    pub length: u64,
    pub region_type: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RuntimeMemoryMapV1 {
    pub protocol_magic: u32,
    pub protocol_version: u32,
    pub cpu_count: u32,
    pub region_count: u32,
    pub regions_ptr: *const RuntimeMemoryRegionV1,
}

impl RuntimeMemoryMapV1 {
    pub fn is_valid(&self) -> bool {
        self.protocol_magic == RUNTIME_INFO_V1_MAGIC
            && self.protocol_version == RUNTIME_INFO_V1_VERSION
            && self.cpu_count > 0
            && self.cpu_count <= RUNTIME_MAX_CPU_COUNT
            && self.region_count > 0
            && self.region_count <= RUNTIME_MEMORY_MAP_MAX_REGIONS
            && !self.regions_ptr.is_null()
    }
}

pub fn native_info_from_summary(summary: RuntimeMemoryMapSummary) -> Option<NativeRuntimeInfoV1> {
    if !summary.is_valid() {
        return None;
    }
    Some(NativeRuntimeInfoV1 {
        protocol_magic: RUNTIME_INFO_V1_MAGIC,
        protocol_version: RUNTIME_INFO_V1_VERSION,
        total_memory_bytes: summary.total_memory_bytes,
        cpu_count: summary.cpu_count,
        memory_map_entries: summary.memory_map_entries,
    })
}

pub fn multiboot2_info_from_summary(
    summary: RuntimeMemoryMapSummary,
) -> Option<Multiboot2RuntimeInfoV1> {
    if !summary.is_valid() {
        return None;
    }
    Some(Multiboot2RuntimeInfoV1 {
        protocol_magic: RUNTIME_INFO_V1_MAGIC,
        protocol_version: RUNTIME_INFO_V1_VERSION,
        total_memory_bytes: summary.total_memory_bytes,
        cpu_count: summary.cpu_count,
        memory_map_entries: summary.memory_map_entries,
    })
}

pub fn stivale2_info_from_summary(summary: RuntimeMemoryMapSummary) -> Option<Stivale2RuntimeInfoV1> {
    if !summary.is_valid() {
        return None;
    }
    Some(Stivale2RuntimeInfoV1 {
        protocol_magic: RUNTIME_INFO_V1_MAGIC,
        protocol_version: RUNTIME_INFO_V1_VERSION,
        total_memory_bytes: summary.total_memory_bytes,
        cpu_count: summary.cpu_count,
        memory_map_entries: summary.memory_map_entries,
    })
}

pub fn summary_from_source<S: RuntimeMemoryMapSource>(source: &S) -> Option<RuntimeMemoryMapSummary> {
    let summary = source.to_summary();
    if summary.is_valid() {
        Some(summary)
    } else {
        None
    }
}

pub unsafe fn summary_from_memory_map_ptr(ptr: *const RuntimeMemoryMapV1) -> Option<RuntimeMemoryMapSummary> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees valid pointer for one read.
    let map = unsafe { core::ptr::read(ptr) };
    if !map.is_valid() {
        return None;
    }
    // SAFETY: caller guarantees valid contiguous memory map array.
    let regions = unsafe { core::slice::from_raw_parts(map.regions_ptr, map.region_count as usize) };
    let mut total = 0u64;
    let mut entries = 0u32;
    for region in regions {
        if region.length == 0 {
            continue;
        }
        if region.base.checked_add(region.length).is_none() {
            return None;
        }
        entries = entries.saturating_add(1);
        if region.region_type == MEMORY_REGION_USABLE {
            total = total.checked_add(region.length)?;
        }
    }
    if total == 0 || entries == 0 {
        return None;
    }
    Some(RuntimeMemoryMapSummary {
        total_memory_bytes: total,
        cpu_count: map.cpu_count,
        memory_map_entries: entries,
    })
}

impl Multiboot2RuntimeInfoV1 {
    pub fn is_valid(&self) -> bool {
        self.protocol_magic == RUNTIME_INFO_V1_MAGIC
            && self.protocol_version == RUNTIME_INFO_V1_VERSION
            && self.total_memory_bytes > 0
            && self.cpu_count > 0
            && self.cpu_count <= RUNTIME_MAX_CPU_COUNT
            && self.memory_map_entries > 0
            && self.memory_map_entries <= RUNTIME_MAX_MEMORY_MAP_ENTRIES
    }
}

impl Stivale2RuntimeInfoV1 {
    pub fn is_valid(&self) -> bool {
        self.protocol_magic == RUNTIME_INFO_V1_MAGIC
            && self.protocol_version == RUNTIME_INFO_V1_VERSION
            && self.total_memory_bytes > 0
            && self.cpu_count > 0
            && self.cpu_count <= RUNTIME_MAX_CPU_COUNT
            && self.memory_map_entries > 0
            && self.memory_map_entries <= RUNTIME_MAX_MEMORY_MAP_ENTRIES
    }
}

impl NativeRuntimeInfoV1 {
    pub fn is_valid(&self) -> bool {
        self.protocol_magic == RUNTIME_INFO_V1_MAGIC
            && self.protocol_version == RUNTIME_INFO_V1_VERSION
            && self.total_memory_bytes > 0
            && self.cpu_count > 0
            && self.cpu_count <= RUNTIME_MAX_CPU_COUNT
            && self.memory_map_entries > 0
            && self.memory_map_entries <= RUNTIME_MAX_MEMORY_MAP_ENTRIES
    }
}

pub unsafe fn snapshot_from_multiboot2_ptr(ptr: *const Multiboot2RuntimeInfoV1) -> Option<RuntimeSnapshot> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees valid pointer for one read.
    let info = unsafe { core::ptr::read(ptr) };
    if !info.is_valid() {
        return None;
    }
    Some(RuntimeSnapshot {
        memory_bytes: info.total_memory_bytes,
        cpu_count: info.cpu_count,
        memory_map_entries: info.memory_map_entries,
    })
}

pub unsafe fn snapshot_from_stivale2_ptr(ptr: *const Stivale2RuntimeInfoV1) -> Option<RuntimeSnapshot> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees valid pointer for one read.
    let info = unsafe { core::ptr::read(ptr) };
    if !info.is_valid() {
        return None;
    }
    Some(RuntimeSnapshot {
        memory_bytes: info.total_memory_bytes,
        cpu_count: info.cpu_count,
        memory_map_entries: info.memory_map_entries,
    })
}

pub unsafe fn snapshot_from_native_ptr(ptr: *const NativeRuntimeInfoV1) -> Option<RuntimeSnapshot> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees valid pointer for one read.
    let info = unsafe { core::ptr::read(ptr) };
    if !info.is_valid() {
        return None;
    }
    Some(RuntimeSnapshot {
        memory_bytes: info.total_memory_bytes,
        cpu_count: info.cpu_count,
        memory_map_entries: info.memory_map_entries,
    })
}
