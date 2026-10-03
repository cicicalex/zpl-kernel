//! The xHCI controller's register window: mapped once, then read and written by offset.
//!
//! # Safety
//!
//! All the `unsafe` that touches the controller's registers is in this file.
//! [`RegisterWindow::map`] is `unsafe` because only its caller can know that the
//! physical range is the controller's BAR. Once it has succeeded, a read or a write is
//! checked against the length that was mapped and done volatile, so the rest of the
//! driver reaches the registers through safe calls and cannot step outside the window
//! by getting an offset wrong.

use crate::mm::paging;

/// Where the window is mapped: the top quarter of the 2 MiB region `paging` manages,
/// far above the ring-3 demo's code and stack (the first 160 KiB) and the e1000's
/// register window (`+0x10_0000`, eight pages).
const VIRT_BASE: u64 = paging::USER_REGION_BASE + 0x18_0000;
const PAGE: u64 = 4096;
/// The most this window may cover. A controller's registers are 64 KiB on the larger
/// real parts and 16 KiB in QEMU; 128 pages is room for the largest of them, and the
/// region above `VIRT_BASE` holds no more.
pub const MAX_LEN: u64 = 0x8_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The base is not page-aligned, or the length is zero or over [`MAX_LEN`].
    BadRange,
    /// `paging` refused one of the pages.
    Refused,
}

/// A mapped register window. Only one exists at a time: mapping again moves the
/// window to the new controller, and the old one must not be used after that, which
/// the driver ensures by only ever holding the latest.
pub struct RegisterWindow {
    len: u64,
}

impl RegisterWindow {
    /// Map `len` bytes of device memory starting at the physical address `phys`.
    ///
    /// # Safety
    /// `phys..phys + len` must lie inside a memory BAR read from the configuration
    /// space of the xHCI controller this driver is bringing up, so the pages are that
    /// controller's registers and not RAM or another device's. Nothing else in the
    /// kernel may use the virtual pages at `VIRT_BASE`, which is true by construction:
    /// this file is the only user of the constant.
    pub unsafe fn map(phys: u64, len: u64) -> Result<Self, MapError> {
        if phys % PAGE != 0 || len == 0 || len > MAX_LEN {
            return Err(MapError::BadRange);
        }
        let pages = len.div_ceil(PAGE);
        for page in 0..pages {
            // SAFETY: the caller's contract -- `phys + page * PAGE` is inside the
            // controller's BAR -- and the virtual page is inside this window, which no
            // one else maps.
            unsafe {
                paging::map_4k_mmio_page(VIRT_BASE + page * PAGE, phys + page * PAGE)
                    .map_err(|_| MapError::Refused)?;
            }
        }
        Ok(Self { len: pages * PAGE })
    }

    /// The number of bytes mapped.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    fn in_window(&self, offset: u64) -> bool {
        offset % 4 == 0 && offset + 4 <= self.len
    }

    /// Read a 32-bit register. An offset outside the window reads as all ones, which
    /// is what an absent device answers too, so the caller's own checks treat it as
    /// "nothing there" rather than as a value.
    #[must_use]
    pub fn read32(&self, offset: u64) -> u32 {
        if !self.in_window(offset) {
            return u32::MAX;
        }
        // SAFETY: the window was mapped by `map`, the offset is aligned and inside
        // it. Volatile, because this is a device register: the compiler must not
        // cache, merge or drop the read.
        unsafe { core::ptr::read_volatile((VIRT_BASE + offset) as *const u32) }
    }

    /// Write a 32-bit register. A write outside the window is dropped.
    pub fn write32(&self, offset: u64, value: u32) {
        if !self.in_window(offset) {
            return;
        }
        // SAFETY: as for `read32`. Volatile for the same reason, and so the store is
        // not reordered with the ones around it.
        unsafe { core::ptr::write_volatile((VIRT_BASE + offset) as *mut u32, value) }
    }

    /// Write a 64-bit register as two 32-bit halves, low first. The specification
    /// allows this for every 64-bit register the driver writes, and it works on
    /// controllers that do not take 64-bit accesses.
    pub fn write64(&self, offset: u64, value: u64) {
        self.write32(offset, (value & 0xFFFF_FFFF) as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }
}
