use core::cell::UnsafeCell;

use crate::boot::BootInfo;

#[derive(Debug, Clone, Copy)]
pub struct CpuInfo {
    pub apic_id: u32,
    pub features: u64,
}

impl CpuInfo {
    pub const fn minimal() -> Self {
        Self {
            apic_id: 0,
            features: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BootFrame {
    pub magic: u64,
    pub lower_memory_kib: u32,
    pub upper_memory_kib: u32,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

impl BootFrame {
    pub const fn minimal() -> Self {
        Self {
            magic: 0x5A_50_4C_4B_45_52_4E_4C, // "ZPLKERNL"
            lower_memory_kib: 640,
            upper_memory_kib: 64 * 1024,
            cpu_count: 1,
            memory_map_entries: 1,
        }
    }

    pub const fn total_memory_bytes(&self) -> u64 {
        ((self.lower_memory_kib as u64) + (self.upper_memory_kib as u64)) * 1024
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExternalBootFrameV1 {
    pub magic: u64,
    pub lower_memory_kib: u32,
    pub upper_memory_kib: u32,
    pub cpu_count: u32,
    pub memory_map_entries: u32,
}

impl ExternalBootFrameV1 {
    pub const PROTOCOL_MAGIC: u64 = 0x5A_50_4C_4B_45_52_4E_4C; // "ZPLKERNL"

    pub fn is_valid(&self) -> bool {
        self.magic == Self::PROTOCOL_MAGIC
            && (self.lower_memory_kib > 0 || self.upper_memory_kib > 0)
            && self.cpu_count > 0
            && self.memory_map_entries > 0
    }

    pub const fn total_memory_bytes(&self) -> u64 {
        ((self.lower_memory_kib as u64) + (self.upper_memory_kib as u64)) * 1024
    }

    pub const fn minimal() -> Self {
        Self {
            magic: Self::PROTOCOL_MAGIC,
            lower_memory_kib: 640,
            upper_memory_kib: 64 * 1024,
            cpu_count: 1,
            memory_map_entries: 1,
        }
    }
}

pub fn parse_boot_frame(frame: BootFrame) -> BootInfo {
    BootInfo {
        boot_magic: frame.magic,
        memory_map_entries: frame.memory_map_entries,
        memory_bytes: frame.total_memory_bytes(),
        cpu_count: frame.cpu_count.max(1),
    }
}

pub fn parse_external_boot_frame(frame: ExternalBootFrameV1) -> Option<BootInfo> {
    if !frame.is_valid() {
        return None;
    }

    Some(BootInfo {
        boot_magic: frame.magic,
        memory_map_entries: frame.memory_map_entries,
        memory_bytes: frame.total_memory_bytes(),
        cpu_count: frame.cpu_count.max(1),
    })
}

/// Adapter entrypoint for an external bootloader frame pointer.
/// Returns `None` when pointer is null or frame is invalid.
///
/// # Safety
/// `frame_ptr` must be valid for reading one `ExternalBootFrameV1`.
pub unsafe fn collect_boot_info_from_ptr(frame_ptr: *const ExternalBootFrameV1) -> Option<BootInfo> {
    if frame_ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees pointer validity for one frame read.
    let frame = unsafe { core::ptr::read(frame_ptr) };
    parse_external_boot_frame(frame)
}

struct ExternalBootFrameSlot(UnsafeCell<ExternalBootFrameV1>);

// Single-writer at boot (Limine bridge); readers use returned `*const` only after publish.
unsafe impl Sync for ExternalBootFrameSlot {}

static EXTERNAL_BOOT_FRAME_V1: ExternalBootFrameSlot = ExternalBootFrameSlot(UnsafeCell::new(
    ExternalBootFrameV1 {
        magic: ExternalBootFrameV1::PROTOCOL_MAGIC,
        lower_memory_kib: 640,
        upper_memory_kib: 64 * 1024,
        cpu_count: 1,
        memory_map_entries: 1,
    },
));

/// Loader-side publisher helper for boot protocol v1.
/// Writes a frame to static shared slot and returns stable pointer
/// that can be passed to `zpl_boot_entry_v1`.
///
/// Takes `&ExternalBootFrameV1` (not by value): the Limine ISO path still hung after `[BRIDGE-PUB-IN]`
/// when this used by-value `ExternalBootFrameV1` + `static mut` assign; `&` avoids struct-arg ABI
/// edge cases at the bare `_start` stack boundary.
pub fn publish_external_boot_frame_v1(frame: &ExternalBootFrameV1) -> *const ExternalBootFrameV1 {
    let p = EXTERNAL_BOOT_FRAME_V1.0.get();
    // Limine ISO + release+LTO: bulk `memcpy`/struct assign into this slot trapped QEMU (serial
    // stopped after `[BRIDGE-PUB-IN]`). Byte-wise volatile copy avoids the bad intrinsic.
    const SZ: usize = core::mem::size_of::<ExternalBootFrameV1>();
    unsafe {
        let dst = p.cast::<u8>();
        let src = (frame as *const ExternalBootFrameV1).cast::<u8>();
        for i in 0..SZ {
            core::ptr::write_volatile(dst.add(i), core::ptr::read_volatile(src.add(i)));
        }
    }
    p.cast_const()
}

/// Convenience publisher for minimal default frame.
pub fn publish_minimal_external_boot_frame_v1() -> *const ExternalBootFrameV1 {
    let f = ExternalBootFrameV1::minimal();
    publish_external_boot_frame_v1(&f)
}

pub fn collect_boot_info() -> BootInfo {
    parse_boot_frame(BootFrame::minimal())
}

pub fn halt_loop() -> ! {
    loop {
        halt_once();
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn halt_once() {
    // SAFETY: `hlt` is valid in kernel mode and used as idle wait.
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
pub fn halt_once() {
    core::hint::spin_loop();
}

#[cfg(target_arch = "x86_64")]
pub fn detect_current_source_id() -> u16 {
    // CPUID leaf 1, EBX[31:24] carries initial APIC ID on x86_64.
    let result = core::arch::x86_64::__cpuid(1);
    ((result.ebx >> 24) & 0xff) as u16
}

#[cfg(not(target_arch = "x86_64"))]
pub fn detect_current_source_id() -> u16 {
    0
}
