//! Pages of memory the controller reads and writes on its own.
//!
//! A [`DmaPage`] is one 4 KiB frame, taken from the frame allocator, zeroed, and
//! never given back -- the keyboard lives as long as the kernel does. The controller
//! is handed its physical address; the CPU reaches it through
//! [`crate::mm::phys_hhdm::pa_to_kernel_va`], which is the same number on the
//! multiboot path and the higher-half window on the Limine one. The e1000 driver
//! learned that distinction the hard way; see `e1000::cpu_addr`.
//!
//! # Safety
//!
//! All the `unsafe` that touches DMA memory is in this file, and none of it is exposed:
//! a page comes straight from the frame allocator, so nothing else owns it; it is
//! refused if the CPU cannot reach it; and every access is volatile and checked against
//! the page's bounds and alignment. The rest of the driver therefore uses safe calls
//! and cannot reach outside its own pages.

/// How far up physical memory the multiboot path's identity map reaches. A frame above
/// it is refused there, because the CPU could not reach it.
const IDENTITY_MAP_TOP: u64 = 1 << 30;
pub const PAGE: usize = 4096;

pub struct DmaPage {
    phys: u64,
    virt: u64,
}

impl DmaPage {
    /// A fresh, zeroed page, or `None` if there is no frame or the CPU cannot reach it.
    #[must_use]
    pub fn alloc() -> Option<Self> {
        let phys = crate::mm::frame_alloc::alloc_frame()?;
        if crate::mm::phys_hhdm::current_hhdm_offset().is_none() && phys >= IDENTITY_MAP_TOP {
            return None;
        }
        let virt = crate::mm::phys_hhdm::pa_to_kernel_va(phys);
        // SAFETY: the frame was just handed out by the allocator, so nothing else
        // owns it, and `virt` is where the CPU reaches it (checked just above for the
        // identity-mapped path). Zeroing a whole frame stays inside it.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, PAGE) };
        Some(Self { phys, virt })
    }

    /// The address the controller uses.
    #[must_use]
    pub fn phys(&self) -> u64 {
        self.phys
    }

    fn fits(offset: usize, width: usize) -> bool {
        offset % width == 0 && offset + width <= PAGE
    }

    #[must_use]
    pub fn read32(&self, offset: usize) -> u32 {
        if !Self::fits(offset, 4) {
            return 0;
        }
        // SAFETY: inside this page, which this value owns, and aligned. Volatile,
        // because the controller writes this memory behind the compiler's back.
        unsafe { core::ptr::read_volatile((self.virt + offset as u64) as *const u32) }
    }

    pub fn write32(&self, offset: usize, value: u32) {
        if !Self::fits(offset, 4) {
            return;
        }
        // SAFETY: as for `read32`; volatile so the controller sees the stores in the
        // order they are written.
        unsafe { core::ptr::write_volatile((self.virt + offset as u64) as *mut u32, value) }
    }

    #[must_use]
    pub fn read64(&self, offset: usize) -> u64 {
        u64::from(self.read32(offset)) | (u64::from(self.read32(offset + 4)) << 32)
    }

    pub fn write64(&self, offset: usize, value: u64) {
        self.write32(offset, (value & 0xFFFF_FFFF) as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }

    #[must_use]
    pub fn read8(&self, offset: usize) -> u8 {
        let word = self.read32(offset & !3);
        (word >> ((offset & 3) * 8)) as u8
    }

    /// Copy bytes out of the page, as many as fit in `out` and in the page.
    pub fn read_bytes(&self, offset: usize, out: &mut [u8]) {
        for (i, byte) in out.iter_mut().enumerate() {
            if offset + i >= PAGE {
                break;
            }
            *byte = self.read8(offset + i);
        }
    }

    /// Zero the whole page again.
    pub fn clear(&self) {
        for offset in (0..PAGE).step_by(4) {
            self.write32(offset, 0);
        }
    }
}
