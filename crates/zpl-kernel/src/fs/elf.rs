//! Minimal ELF64 loader for ring-3 user images.
//!
//! What it does:
//!
//! - Parses an ELF64 header and program headers from an in-memory blob.
//! - For every `PT_LOAD` segment, allocates physical frames, copies the
//!   bytes from the file image, zero-fills the BSS gap, and maps the
//!   resulting pages user-accessible via `paging::map_4k_user_page`.
//! - Returns the entry-point virtual address so the caller can build an
//!   `iretq` frame.
//!
//! What it does NOT do (out of scope for v1):
//!
//! - PT_DYNAMIC, PT_TLS, PT_GNU_RELRO, relocations.
//! - PIE / position-independent loading. The image must be linked at the
//!   target virtual address (`USER_REGION_BASE + offset`).
//! - Symbol resolution / shared libraries.
//!
//! Parses ELF64 per the TIS specification.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::ptr::read_unaligned;

use crate::mm::frame_alloc::alloc_frame;
use crate::mm::paging;

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELF_CLASS_64: u8 = 2;
const ELF_DATA_LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 0x3E;
const PT_LOAD: u32 = 1;
const PAGE_SIZE: u64 = 4096;

#[derive(Debug, Clone, Copy)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    BadClass,
    BadData,
    BadType,
    BadMachine,
    PhdrOutOfRange,
    SegmentOutOfRange,
    NoFrame,
    MapFailed,
    BadAlignment,
}

#[derive(Debug, Clone, Copy)]
pub struct LoadedImage {
    pub entry: u64,
    pub load_count: u32,
}

#[repr(C, packed)]
struct Elf64Ehdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C, packed)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

/// Parse `bytes` as ELF64 and load every `PT_LOAD` segment into the
/// user-region paging slot. Returns the entry virtual address.
///
/// # Safety
/// Caller must guarantee that the segments do not collide with already
/// mapped user pages (currently we only have the ring 3 demo region).
pub unsafe fn load(bytes: &[u8]) -> Result<LoadedImage, ElfError> {
    if bytes.len() < core::mem::size_of::<Elf64Ehdr>() {
        return Err(ElfError::TooSmall);
    }

    // SAFETY: bounds checked above, repr(C, packed) aligns u8.
    let ehdr = unsafe { read_unaligned(bytes.as_ptr() as *const Elf64Ehdr) };

    if ehdr.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }
    if ehdr.e_ident[4] != ELF_CLASS_64 {
        return Err(ElfError::BadClass);
    }
    if ehdr.e_ident[5] != ELF_DATA_LSB {
        return Err(ElfError::BadData);
    }
    let e_type = ehdr.e_type;
    if e_type != ET_EXEC {
        return Err(ElfError::BadType);
    }
    let e_machine = ehdr.e_machine;
    if e_machine != EM_X86_64 {
        return Err(ElfError::BadMachine);
    }

    let phoff = ehdr.e_phoff as usize;
    let phentsize = ehdr.e_phentsize as usize;
    let phnum = ehdr.e_phnum as usize;
    let phdr_table_end = phoff
        .checked_add(phentsize.checked_mul(phnum).ok_or(ElfError::PhdrOutOfRange)?)
        .ok_or(ElfError::PhdrOutOfRange)?;
    if phdr_table_end > bytes.len() {
        return Err(ElfError::PhdrOutOfRange);
    }

    let mut load_count: u32 = 0;
    for idx in 0..phnum {
        let phdr_off = phoff + idx * phentsize;
        // SAFETY: bounds checked above.
        let phdr = unsafe {
            read_unaligned(bytes.as_ptr().add(phdr_off) as *const Elf64Phdr)
        };
        if phdr.p_type != PT_LOAD {
            continue;
        }
        unsafe { map_segment(bytes, &phdr)? };
        load_count = load_count.saturating_add(1);
    }

    Ok(LoadedImage {
        entry: ehdr.e_entry,
        load_count,
    })
}

unsafe fn map_segment(bytes: &[u8], phdr: &Elf64Phdr) -> Result<(), ElfError> {
    let p_offset = phdr.p_offset as usize;
    let p_filesz = phdr.p_filesz as usize;
    let p_memsz = phdr.p_memsz as usize;
    let p_vaddr = phdr.p_vaddr;

    if p_offset
        .checked_add(p_filesz)
        .ok_or(ElfError::SegmentOutOfRange)?
        > bytes.len()
    {
        return Err(ElfError::SegmentOutOfRange);
    }
    if p_filesz > p_memsz {
        return Err(ElfError::SegmentOutOfRange);
    }
    if p_vaddr < paging::USER_REGION_BASE
        || p_vaddr + p_memsz as u64 > paging::USER_REGION_BASE + 0x20_0000
    {
        return Err(ElfError::SegmentOutOfRange);
    }

    // Walk page-aligned virtual range that covers the segment.
    let seg_start = p_vaddr & !(PAGE_SIZE - 1);
    let seg_end = (p_vaddr + p_memsz as u64).div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let mut written: u64 = 0;
    let mut virt = seg_start;
    while virt < seg_end {
        let frame = alloc_frame().ok_or(ElfError::NoFrame)?;
        // Where the kernel can reach this frame. On the multiboot path low
        // physical memory is identity-mapped and this is the frame address itself;
        // on the Limine path the kernel is in the higher half and reaches physical
        // memory through the HHDM window.
        //
        // This line used to write to `frame` directly, with a comment asserting the
        // identity map. On the ISO that address is not mapped, so the write stopped
        // the machine -- no fault message, no output, the halt-loop tick simply
        // ended. Found with markers, narrowed to this statement: the marker before
        // the allocation printed, the one after the zero-fill never did.
        let dst = crate::mm::phys_hhdm::pa_to_kernel_va(frame);
        unsafe {
            // Zero-fill the whole page first so the BSS portion is clean.
            core::ptr::write_bytes(dst as *mut u8, 0, PAGE_SIZE as usize);
        }

        // Copy the slice of bytes belonging to this page from the ELF
        // image. We compute overlap between [seg_start..seg_start+p_filesz]
        // and the current page.
        let page_lo = virt;
        let page_hi = virt + PAGE_SIZE;
        let copy_lo = core::cmp::max(page_lo, p_vaddr);
        let copy_hi = core::cmp::min(page_hi, p_vaddr + p_filesz as u64);
        if copy_lo < copy_hi {
            let inside_offset = (copy_lo - p_vaddr) as usize;
            let len = (copy_hi - copy_lo) as usize;
            let src = &bytes[p_offset + inside_offset..p_offset + inside_offset + len];
            let dst_off = (copy_lo - page_lo) as usize;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    (dst as usize + dst_off) as *mut u8,
                    len,
                );
            }
            written = written.saturating_add(len as u64);
        }

        unsafe { paging::map_4k_user_page(virt, frame).map_err(|_| ElfError::MapFailed)? };
        virt += PAGE_SIZE;
    }
    let _ = written;
    Ok(())
}
