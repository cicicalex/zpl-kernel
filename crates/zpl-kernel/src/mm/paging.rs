//! 4 KiB virtual-memory mapping over the bootstrap PML4.
//!
//! The bootstrap assembly in `start.rs` already sets up:
//! - `zpl_pml4[0] -> zpl_pdpt`
//! - `zpl_pdpt[0] -> zpl_pd` (with PS=1 for 2 MiB pages spanning [0, 1 GiB))
//!
//! This module owns the second slot:
//! - `zpl_pdpt[1] -> PAGING_PD` (4 KiB granularity)
//! - `PAGING_PD[0] -> PAGING_PT`
//!
//! As a result, virtual addresses in `[USER_REGION_BASE, USER_REGION_BASE + 2 MiB)`
//! are managed by `map_4k_page` / `unmap_page`. Anything outside this range is
//! left untouched (still identity-mapped 2 MiB by the bootstrap).
//!
//! **Limine:** the firmware’s active PML4 may not reference our static `zpl_pdpt`; after wiring
//! `PAGING_PD` / `zpl_pdpt[1]`, [`init`] installs **PML4[0] → `zpl_pdpt`** (via HHDM + CR3) so
//! walks to [`USER_REGION_BASE`] succeed. The Multiboot path already sets this in assembly; the
//! hook is idempotent when the entry already matches.
//!
//! Per-process address spaces (CR3 swap) are deliberately out of scope here;
//! that is reserved for a later task.
//!
//! Standard x86_64 paging: Intel SDM volume 3, and the OSDev wiki's
//! "Setting Up Paging".

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// `AtomicBool` has exactly one user in this file: the VGA-mirror readiness flag
// below, which is compiled out of a `qemu_boot` build. Gated identically, so the
// import does not sit there as an unused-import warning in the default build.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
    feature = "vga_crit_mirror",
))]
use core::sync::atomic::AtomicBool;

use crate::mm::phys_hhdm;

/// First virtual address served by `map_4k_page`. Located at the second
/// 1 GiB slot in PML4[0] so the bootstrap identity map remains intact.
pub const USER_REGION_BASE: u64 = 0x4000_0000;
const USER_REGION_SIZE: u64 = 0x20_0000; // 2 MiB == one PD entry of 4 KiB pages.

const PAGE_SIZE: u64 = 4096;
const PAGE_PRESENT: u64 = 1 << 0;
const PAGE_WRITABLE: u64 = 1 << 1;
const PAGE_USER: u64 = 1 << 2;
const PAGE_BASE_FLAGS: u64 = PAGE_PRESENT | PAGE_WRITABLE;
/// Page-level cache disable. Device registers must not be cached: a write that
/// sits in a cache line is a write the device has not seen, and a read that is
/// served from one is a value the device did not give. Every MMIO mapping sets
/// this; ordinary memory never does.
const PAGE_CACHE_DISABLE: u64 = 1 << 4;
/// Page-level write-through, set alongside PCD by convention for device memory.
const PAGE_WRITE_THROUGH: u64 = 1 << 3;
const PAGE_MMIO_FLAGS: u64 =
    PAGE_PRESENT | PAGE_WRITABLE | PAGE_CACHE_DISABLE | PAGE_WRITE_THROUGH;
/// PDPT[1] / PD[0] are exposed to ring 3 so user-mode code mapped via
/// `map_4k_user_page` can be reached. Per-PT entries decide whether each
/// 4 KiB page is supervisor- or user-accessible.
const PAGE_USER_TABLE_FLAGS: u64 = PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER;

#[repr(C, align(4096))]
struct PageTablePage([u64; 512]);

static mut PAGING_PD: PageTablePage = PageTablePage([0; 512]);
static mut PAGING_PT: PageTablePage = PageTablePage([0; 512]);

/// Dummy frame used by `handle_pf` to satisfy a re-read after `unmap_page`.
/// Calling `init` clears the contents.
#[repr(C, align(4096))]
struct DummyFrame([u8; 4096]);
static mut DUMMY_FRAME: DummyFrame = DummyFrame([0; 4096]);

pub static PAGE_FAULT_COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_PF_ADDR: AtomicU64 = AtomicU64::new(0);

/// Set after [`map_vga_text_buffer_hhdm`] installs a leaf for `HHDM + 0xB8000`. Until then,
/// `emit_critical_marker` must not touch MMIO at that VA (Limine leaves it unmapped).
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
    feature = "vga_crit_mirror",
))]
static VGA_HHDM_MAP_READY: AtomicBool = AtomicBool::new(false);

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
    feature = "vga_crit_mirror",
))]
#[inline]
pub fn is_vga_hhdm_mapped() -> bool {
    VGA_HHDM_MAP_READY.load(Ordering::Acquire)
}

#[cfg(not(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
    feature = "vga_crit_mirror",
)))]
#[inline]
pub fn is_vga_hhdm_mapped() -> bool {
    true
}

#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot")
))]
extern "C" {
    static mut zpl_pdpt: crate::start::ZplPdptPage;
}

#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot"))]
extern "C" {
    static mut zpl_pdpt: [u64; 512];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingError {
    OutOfRegion,
    Misaligned,
}

#[inline]
fn invlpg(addr: u64) {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    unsafe {
        core::arch::asm!(
            "invlpg [{addr}]",
            addr = in(reg) addr,
            options(nostack, preserves_flags),
        );
    }
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    {
        let _ = addr;
    }
}

#[inline]
pub fn set_hhdm_offset(offset: u64) {
    phys_hhdm::set_hhdm_offset(offset);
}

/// Limine executable virtual/physical base (see [`phys_hhdm::set_kernel_image_offset`]).
#[inline]
pub fn set_kernel_image_offset(virt_base: u64, phys_base: u64) {
    phys_hhdm::set_kernel_image_offset(virt_base, phys_base);
}

#[inline]
fn read_cr2() -> u64 {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    unsafe {
        let value: u64;
        core::arch::asm!("mov {0}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
        value
    }
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    {
        0
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline]
unsafe fn read_cr3() -> u64 {
    let value: u64;
    core::arch::asm!(
        "mov {0}, cr3",
        out(reg) value,
        options(nomem, nostack, preserves_flags),
    );
    value
}

#[inline]
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
))]
fn pte_table_phys(entry: u64) -> u64 {
    entry & 0x000F_FFFF_FFFF_F000u64
}

/// Map physical `0xB8000` (VGA text buffer) at `HHDM + 0xB8000` for Limine: HHDM often
/// omits legacy MMIO, so `vga_crit_mirror` would otherwise #PF on first write.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
))]
unsafe fn map_vga_text_buffer_hhdm() {
    const MARKER: &[u8] = b"[ZPL-PAGING] vga 0xB8000 mapped via HHDM\n";
    const VGA_PHYS: u64 = 0xB8000;
    const TABLE_RW: u64 = PAGE_PRESENT | PAGE_WRITABLE;

    let hhdm_off = match phys_hhdm::current_hhdm_offset() {
        Some(v) => v,
        None => return,
    };
    let target_va = hhdm_off.wrapping_add(VGA_PHYS);

    let pml4_idx = ((target_va >> 39) & 0x1ff) as usize;
    let pdpt_idx = ((target_va >> 30) & 0x1ff) as usize;
    let pd_idx = ((target_va >> 21) & 0x1ff) as usize;
    let pt_idx = ((target_va >> 12) & 0x1ff) as usize;

    let cr3 = read_cr3();
    let pml4_phys = cr3 & !0xfffu64;
    let pml4 = hhdm_off.wrapping_add(pml4_phys) as *mut u64;

    let mut pml4_e = core::ptr::read_volatile(pml4.add(pml4_idx));
    if pml4_e & 1 == 0 {
        let Some(new_phys) = crate::mm::frame_alloc::alloc_frame() else {
            return;
        };
        let new_va = hhdm_off.wrapping_add(new_phys) as *mut u8;
        core::ptr::write_bytes(new_va, 0, 4096);
        pml4_e = new_phys | TABLE_RW;
        core::ptr::write_volatile(pml4.add(pml4_idx), pml4_e);
    } else if (pml4_e & (1 << 7)) != 0 {
        return;
    }

    let pdpt_phys = pte_table_phys(pml4_e);
    let pdpt = hhdm_off.wrapping_add(pdpt_phys) as *mut u64;

    let mut pdpt_e = core::ptr::read_volatile(pdpt.add(pdpt_idx));
    if pdpt_e & 1 == 0 {
        let Some(new_phys) = crate::mm::frame_alloc::alloc_frame() else {
            return;
        };
        let new_va = hhdm_off.wrapping_add(new_phys) as *mut u8;
        core::ptr::write_bytes(new_va, 0, 4096);
        pdpt_e = new_phys | TABLE_RW;
        core::ptr::write_volatile(pdpt.add(pdpt_idx), pdpt_e);
    } else if (pdpt_e & (1 << 7)) != 0 {
        return;
    }

    let pd_phys = pte_table_phys(pdpt_e);
    let pd = hhdm_off.wrapping_add(pd_phys) as *mut u64;

    let mut pd_e = core::ptr::read_volatile(pd.add(pd_idx));
    if pd_e & 1 != 0 && (pd_e & (1 << 7)) != 0 {
        let base_2m = pd_e & 0x000F_FFFFFFE00000u64;
        let mapped_page_phys = base_2m | (target_va & 0x1FF000);
        if mapped_page_phys == VGA_PHYS {
            invlpg(target_va);
            #[cfg(feature = "vga_crit_mirror")]
            VGA_HHDM_MAP_READY.store(true, Ordering::Release);
            crate::drivers::console::boot_probe_slice_no_vga(MARKER);
        }
        return;
    }

    if pd_e & 1 == 0 {
        let Some(new_phys) = crate::mm::frame_alloc::alloc_frame() else {
            return;
        };
        let new_va = hhdm_off.wrapping_add(new_phys) as *mut u8;
        core::ptr::write_bytes(new_va, 0, 4096);
        pd_e = new_phys | TABLE_RW;
        core::ptr::write_volatile(pd.add(pd_idx), pd_e);
    }

    let pt_phys = pte_table_phys(pd_e);
    let pt = hhdm_off.wrapping_add(pt_phys) as *mut u64;

    let leaf = VGA_PHYS | TABLE_RW;
    core::ptr::write_volatile(pt.add(pt_idx), leaf);
    invlpg(target_va);

    #[cfg(feature = "vga_crit_mirror")]
    VGA_HHDM_MAP_READY.store(true, Ordering::Release);
    crate::drivers::console::boot_probe_slice_no_vga(MARKER);
}

/// One-line COM1 trace: virtual address of `zpl_pdpt` (expect `addr & 0xFFF == 0` after align fix).
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn emit_pdpt_va_diag() {
    let pdpt_va = core::ptr::addr_of!(zpl_pdpt) as usize as u64;
    // One buffer, one marker: written a nibble at a time this line reached COM1
    // only, so it was the one boot line a reader could never see on the screen.
    let mut line = crate::drivers::console::MarkerLine::new();
    line.push(b"[ZPL-PDPT-VA=");
    line.push_hex64(pdpt_va);
    line.push(b"]\n");
    crate::drivers::console::emit_critical_marker(line.as_slice());
}

/// Wire `PAGING_PD` and `PAGING_PT` into the bootstrap PDPT slot 1.
///
/// # Safety
/// Must run exactly once on the bootstrap CPU before any caller touches
/// `[USER_REGION_BASE, USER_REGION_BASE + USER_REGION_SIZE)`. After this
/// function returns the page tables remain visible to the CPU until the
/// kernel halts.
pub unsafe fn init() {
    const MARKER_INIT: &[u8] = b"[ZPL-PAGING] init pdpt[1]+pd[0] ok\n";
    const MARKER_PML4: &[u8] = b"[ZPL-PAGING] pml4[0] hooked\n";

    unsafe {
        for slot in (&mut *core::ptr::addr_of_mut!(PAGING_PT)).0.iter_mut() {
            *slot = 0;
        }
        for slot in (&mut *core::ptr::addr_of_mut!(PAGING_PD)).0.iter_mut() {
            *slot = 0;
        }
        for slot in (&mut *core::ptr::addr_of_mut!(DUMMY_FRAME)).0.iter_mut() {
            *slot = 0;
        }
    }

    #[cfg(all(target_os = "none", target_arch = "x86_64", not(feature = "qemu_boot")))]
    unsafe {
        map_vga_text_buffer_hhdm();
    }

    let pt_phys = paging_pt_phys();
    let pd_phys = paging_pd_phys();
    unsafe {
        // SAFETY: `PAGING_PD` lives in the kernel image mapped by Limine (higher-half VA). We must
        // mutate slot 0 through that VA — not through `pd_phys as *mut` (that assumed identity map
        // from the Multiboot path). `pt_phys` / `pd_phys` are physical addresses encoded into PTEs.
        let pd_first_slot = core::ptr::addr_of_mut!(PAGING_PD).cast::<u64>();
        core::ptr::write_volatile(pd_first_slot, pt_phys | PAGE_USER_TABLE_FLAGS);
        let pdpt_slot1 = core::ptr::addr_of_mut!(zpl_pdpt).cast::<u64>().add(1);
        core::ptr::write_volatile(pdpt_slot1, pd_phys | PAGE_USER_TABLE_FLAGS);
    }

    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    {
        crate::drivers::console::emit_critical_marker(MARKER_INIT);
        emit_pdpt_va_diag();
        unsafe {
            let cr3 = read_cr3();
            let pml4_phys = cr3 & !0xfffu64;
            let pml4_va = match phys_hhdm::current_hhdm_offset() {
                Some(off) => off.wrapping_add(pml4_phys),
                None => pml4_phys,
            };
            let pml4_slot0 = pml4_va as *mut u64;
            let zpl_pdpt_phys =
                phys_hhdm::kernel_va_to_pa(core::ptr::addr_of!(zpl_pdpt) as usize as u64);
            let new_entry = zpl_pdpt_phys | PAGE_USER_TABLE_FLAGS;
            let existing = core::ptr::read_volatile(pml4_slot0);
            if existing != new_entry {
                core::ptr::write_volatile(pml4_slot0, new_entry);
            }
            invlpg(USER_REGION_BASE);
        }
        crate::drivers::console::emit_critical_marker(MARKER_PML4);
    }
}

#[inline]
fn paging_pt_phys() -> u64 {
    phys_hhdm::kernel_va_to_pa(core::ptr::addr_of!(PAGING_PT) as u64)
}

#[inline]
fn paging_pd_phys() -> u64 {
    phys_hhdm::kernel_va_to_pa(core::ptr::addr_of!(PAGING_PD) as u64)
}

#[inline]
fn dummy_frame_phys() -> u64 {
    phys_hhdm::kernel_va_to_pa(core::ptr::addr_of!(DUMMY_FRAME) as u64)
}

fn pt_index_for(virt: u64) -> Result<usize, PagingError> {
    if virt < USER_REGION_BASE || virt >= USER_REGION_BASE + USER_REGION_SIZE {
        return Err(PagingError::OutOfRegion);
    }
    if !virt.is_multiple_of(PAGE_SIZE) {
        return Err(PagingError::Misaligned);
    }
    let pt_idx = ((virt - USER_REGION_BASE) / PAGE_SIZE) as usize;
    Ok(pt_idx)
}

/// Map a 4 KiB virtual page (`virt`) to a physical frame (`phys`) for
/// supervisor-only access (U=0).
///
/// # Safety
/// Caller must guarantee that `phys` is a frame allocated through
/// `crate::mm::frame_alloc::alloc_frame` (or another exclusive owner) so this
/// mapping does not alias the kernel's own image. `virt` must be unique
/// across concurrent mappers.
pub unsafe fn map_4k_page(virt: u64, phys: u64) -> Result<(), PagingError> {
    unsafe { map_4k_page_with_flags(virt, phys, PAGE_BASE_FLAGS) }
}

/// Map a 4 KiB page of **device registers** at `phys`, uncached.
///
/// Separate from [`map_4k_page`] for two reasons, and both matter. The flags
/// differ: device memory is mapped cache-disabled and write-through, because a
/// cached register write is one the device never sees. And the safety contract
/// is the opposite one: `map_4k_page` requires `phys` to be a frame the kernel
/// owns, while this requires that it is **not** — it must be a region a PCI base
/// address register reported, which is not RAM and must never be handed to the
/// frame allocator.
///
/// # Safety
/// `phys` must be inside a memory BAR that was read from a real device's
/// configuration space, and `virt` must be a page nothing else is using. Mapping
/// an address that is not device memory, or that belongs to another device,
/// gives the kernel a window onto something it has no business writing to.
pub unsafe fn map_4k_mmio_page(virt: u64, phys: u64) -> Result<(), PagingError> {
    // SAFETY: the caller's contract, forwarded unchanged; the only difference
    // from the other mappers is the flag set.
    unsafe { map_4k_page_with_flags(virt, phys, PAGE_MMIO_FLAGS) }
}

/// Map a 4 KiB virtual page (`virt`) to `phys` with U=1 so ring 3 can
/// access it (read + write). Used by the ring-3 demo and by future user
/// space loaders.
///
/// # Safety
/// Same constraints as `map_4k_page`. Additionally the caller must accept
/// that the page is now reachable from ring 3.
pub unsafe fn map_4k_user_page(virt: u64, phys: u64) -> Result<(), PagingError> {
    unsafe { map_4k_page_with_flags(virt, phys, PAGE_USER_TABLE_FLAGS) }
}

unsafe fn map_4k_page_with_flags(
    virt: u64,
    phys: u64,
    flags: u64,
) -> Result<(), PagingError> {
    let pt_idx = pt_index_for(virt)?;
    if !phys.is_multiple_of(PAGE_SIZE) {
        return Err(PagingError::Misaligned);
    }
    unsafe {
        let slot = (&raw mut PAGING_PT).cast::<u64>().add(pt_idx);
        core::ptr::write_volatile(slot, phys | flags);
    }
    invlpg(virt);
    Ok(())
}

/// Drop the mapping for `virt`. After return, accessing `virt` will raise
/// a #PF unless `handle_pf` re-installs a placeholder mapping.
///
/// # Safety
/// The same exclusivity assumptions as `map_4k_page` apply.
pub unsafe fn unmap_page(virt: u64) -> Result<(), PagingError> {
    let pt_idx = pt_index_for(virt)?;
    unsafe {
        let slot = (&raw mut PAGING_PT).cast::<u64>().add(pt_idx);
        core::ptr::write_volatile(slot, 0);
    }
    invlpg(virt);
    Ok(())
}

/// Invoked by the IDT vector-14 ISR when a page fault is observed. Implements
/// the "page fault on re-read" acceptance: counts the fault,
/// records CR2, and re-maps the offending page to `DUMMY_FRAME` so the
/// faulting instruction can retry and the kernel can continue.
pub fn handle_pf() {
    let cr2 = read_cr2();
    LAST_PF_ADDR.store(cr2, Ordering::SeqCst);
    PAGE_FAULT_COUNT.fetch_add(1, Ordering::SeqCst);

    if cr2 < USER_REGION_BASE || cr2 >= USER_REGION_BASE + USER_REGION_SIZE {
        return;
    }
    let aligned = cr2 & !(PAGE_SIZE - 1);
    let dummy = dummy_frame_phys();
    let _ = unsafe { map_4k_page(aligned, dummy) };
}

pub fn last_page_fault_addr() -> u64 {
    LAST_PF_ADDR.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfCheckOk {
    pub virt: u64,
    pub phys: u64,
    pub written: u64,
    pub read_back: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    NoFrame,
    MapFailed,
    UnmapFailed,
    Mismatch { expected: u64, got: u64 },
}

/// Acceptance test: map -> write -> read -> verify -> unmap.
///
/// # Safety
/// Must run after `init()` and before any other writer touches
/// `[USER_REGION_BASE, USER_REGION_BASE + 4 KiB)`.
pub unsafe fn self_check_map_write_read_unmap() -> Result<SelfCheckOk, SelfCheckErr> {
    const PATTERN: u64 = 0xCAFEBABE_DEADBEEF;
    let virt = USER_REGION_BASE;
    let phys = match crate::mm::frame_alloc::alloc_frame() {
        Some(p) => p,
        None => return Err(SelfCheckErr::NoFrame),
    };
    if unsafe { map_4k_page(virt, phys) }.is_err() {
        return Err(SelfCheckErr::MapFailed);
    }
    unsafe { core::ptr::write_volatile(virt as *mut u64, PATTERN) };
    let read_back = unsafe { core::ptr::read_volatile(virt as *const u64) };
    if read_back != PATTERN {
        return Err(SelfCheckErr::Mismatch {
            expected: PATTERN,
            got: read_back,
        });
    }
    if unsafe { unmap_page(virt) }.is_err() {
        return Err(SelfCheckErr::UnmapFailed);
    }
    Ok(SelfCheckOk {
        virt,
        phys,
        written: PATTERN,
        read_back,
    })
}

/// Trigger a #PF by reading the page that `self_check_map_write_read_unmap`
/// just unmapped, exercising the IDT vector-14 path.
pub fn trigger_pf_re_read() -> u64 {
    unsafe { core::ptr::read_volatile(USER_REGION_BASE as *const u64) }
}
