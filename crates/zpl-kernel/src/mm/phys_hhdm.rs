//! Higher-half direct map (Limine HHDM) helpers for PTE physical addresses.
//!
//! Under QEMU `-kernel`, the bootstrap page tables operate in an identity-map
//! window so `physical == virtual` for kernel-owned page tables when encoding
//! addresses into PTE slots.
//!
//! After Limine handoff (`protocol: limine`), kernel-visible pointers from
//! [`core::ptr::addr_of!`] typically live inside the HHDM; encode PTE values
//! with `physical = va - HHDM_OFFSET` per the Limine boot protocol.

use core::sync::atomic::{AtomicU64, Ordering};

/// Sentinel: identity mapping (`va_to_pa(va) == va`).
const HHDM_DISABLED: u64 = u64::MAX;

static HHDM_OFFSET_VAL: AtomicU64 = AtomicU64::new(HHDM_DISABLED);

/// `(0, 0)` ⇒ [`kernel_va_to_pa`] behaves like identity (`va` unchanged) — Multiboot / QEMU `-kernel`.
static KERNEL_VIRT_BASE: AtomicU64 = AtomicU64::new(0);
static KERNEL_PHYS_BASE: AtomicU64 = AtomicU64::new(0);

/// Limine `ExecutableAddressRequest`: higher-half virtual base and loaded physical base of this ELF.
///
/// Call from `limine-bridge` after [`set_hhdm_offset`], **before** entering the kernel, so
/// [`kernel_va_to_pa`] can translate addresses of kernel-image statics (e.g. bootstrap page tables)
/// for PTE values. `(0, 0)` leaves identity behaviour for the Multiboot path.
#[inline]
pub fn set_kernel_image_offset(virt_base: u64, phys_base: u64) {
    KERNEL_VIRT_BASE.store(virt_base, Ordering::SeqCst);
    KERNEL_PHYS_BASE.store(phys_base, Ordering::SeqCst);
}

/// Translate a kernel **virtual** address (in the Limine-mapped executable range) to **physical**.
///
/// When both bases are zero (default), returns `va` unchanged (identity), matching the QEMU
/// Multiboot bootstrap. Otherwise: `phys = va - virt_base + phys_base`.
#[must_use]
#[inline]
pub fn kernel_va_to_pa(va: u64) -> u64 {
    let v = KERNEL_VIRT_BASE.load(Ordering::SeqCst);
    let p = KERNEL_PHYS_BASE.load(Ordering::SeqCst);
    if v == 0 && p == 0 {
        va
    } else {
        va.saturating_sub(v).saturating_add(p)
    }
}

/// Returns `Some((virt_base, phys_base))` after a non-default [`set_kernel_image_offset`] install.
#[must_use]
pub fn current_kernel_image_offsets() -> Option<(u64, u64)> {
    let v = KERNEL_VIRT_BASE.load(Ordering::SeqCst);
    let p = KERNEL_PHYS_BASE.load(Ordering::SeqCst);
    if v == 0 && p == 0 {
        None
    } else {
        Some((v, p))
    }
}

/// Install HHDM offset from Limine `HhdmRequest` (`offset` field).
///
/// Call from `limine-bridge` **before** the kernel installs non-bootstrap PTEs,
/// ideally before `crate::mm::paging::init`.
#[inline]
pub fn set_hhdm_offset(offset: u64) {
    HHDM_OFFSET_VAL.store(offset, Ordering::SeqCst);
}

#[inline]
pub fn reset_hhdm_identity() {
    HHDM_OFFSET_VAL.store(HHDM_DISABLED, Ordering::SeqCst);
}

#[must_use]
#[inline]
pub fn current_hhdm_offset() -> Option<u64> {
    let v = HHDM_OFFSET_VAL.load(Ordering::SeqCst);
    if v == HHDM_DISABLED {
        None
    } else {
        Some(v)
    }
}

#[must_use]
#[inline]
pub fn va_to_pa(va: u64) -> u64 {
    match current_hhdm_offset() {
        None => va,
        Some(off) => va.saturating_sub(off),
    }
}

/// The address the kernel can use to reach physical address `pa`.
///
/// On the multiboot path the low physical memory is identity-mapped, so a physical
/// address is already a usable virtual one. On the Limine path it is not: the kernel
/// runs in the higher half and reaches physical memory through the HHDM window. Code
/// that writes to a freshly allocated frame has to ask which world it is in.
///
/// Getting this wrong does not return an error. It writes to an address that is not
/// mapped, and the machine stops -- which is what `load` did at the prompt on the ISO
/// path, measured with markers down to the single statement.
///
/// The `pa < off` guard is what makes it safe to call twice: an address that is
/// already above the window is left alone rather than shifted again.
#[must_use]
#[inline]
pub fn pa_to_kernel_va(pa: u64) -> u64 {
    match current_hhdm_offset() {
        Some(off) if pa < off => off.wrapping_add(pa),
        _ => pa,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three cases in one test on purpose: they all move the same global, and
    /// `cargo test` runs test functions in parallel, so three of them setting and
    /// clearing it would interfere with each other and with the two above.
    #[test]
    fn pa_to_kernel_va_covers_both_worlds() {
        reset_hhdm_identity();
        assert_eq!(pa_to_kernel_va(0x1000), 0x1000, "no window: identity");
        assert_eq!(pa_to_kernel_va(0), 0);

        set_hhdm_offset(0xffff_8000_0000_0000);
        assert_eq!(pa_to_kernel_va(0x1000), 0xffff_8000_0000_1000, "window: shifted");

        // Calling it twice must not shift twice: the second call is given an address
        // that is already above the offset.
        let once = pa_to_kernel_va(0x1000);
        assert_eq!(pa_to_kernel_va(once), once, "idempotent above the window");

        reset_hhdm_identity();
    }

    #[test]
    fn va_to_pa_identity_when_disabled() {
        reset_hhdm_identity();
        assert_eq!(va_to_pa(0x1234_5678_abcd_eff0), 0x1234_5678_abcd_eff0);
    }

    #[test]
    fn va_to_pa_with_hhdm_offset_works() {
        reset_hhdm_identity();
        let off = 0xffff_8000_0000_0000u64;
        set_hhdm_offset(off);
        assert_eq!(
            va_to_pa(0xffff_8001_0201_3040),
            0xffff_8001_0201_3040u64.saturating_sub(off)
        );
        reset_hhdm_identity();
    }

    #[test]
    fn va_to_pa_hhdm_saturates_below_offset() {
        reset_hhdm_identity();
        set_hhdm_offset(0xffff_ff00_0000_0000);
        assert_eq!(va_to_pa(0x1000), 0);
        reset_hhdm_identity();
    }

    #[test]
    fn kernel_va_to_pa_identity_when_unset() {
        set_kernel_image_offset(0, 0);
        assert_eq!(
            kernel_va_to_pa(0xffff_ffff_8012_4000),
            0xffff_ffff_8012_4000
        );
    }

    #[test]
    fn kernel_va_to_pa_limine_style() {
        let virt_base = 0xffff_ffff_8010_0000u64;
        let phys_base = 0xfe9_2000u64;
        set_kernel_image_offset(virt_base, phys_base);
        let va = 0xffff_ffff_8012_4000u64;
        let expected = va - virt_base + phys_base;
        assert_eq!(kernel_va_to_pa(va), expected);
        assert_eq!(current_kernel_image_offsets(), Some((virt_base, phys_base)));
        set_kernel_image_offset(0, 0);
        assert_eq!(current_kernel_image_offsets(), None);
    }
}
