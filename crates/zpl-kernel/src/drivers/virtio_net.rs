//! Virtio network — PCI enumeration + legacy I/O BAR peek (partial).
//!
//! RX/TX virtqueues + ARP land with `#21` / `smoltcp`.  We safely read PCI
//! config space always; when BAR0 is a **legacy I/O BAR** (bit 0 set, as QEMU
//! often reports with `0x…c001`), we also read the virtio legacy **device
//! status** field at I/O offset 18.  **MMIO BARs** are left untouched until the
//! page table can map high PCI aperture.

use crate::drivers::pci::pci_read32;

/// Legacy virtio-pci layout: `VIRTIO_PCI_STATUS` byte offset from I/O base.
const VIRTIO_PCI_LEGACY_STATUS: u16 = 18;

const VIRTIO_PCI_VENDOR: u16 = 0x1AF4;
/// Transitional virtio-net (QEMU default for `virtio-net-pci`).
const VIRTIO_PCI_DEVICE_NET_LEGACY: u16 = 0x1000;
/// Modern virtio-net-only id (some images use this).
const VIRTIO_PCI_DEVICE_NET_MODERN: u16 = 0x1041;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtioNetPciInfo {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    /// Raw BAR0 dword from config offset 0x10 (low bits encode MMIO vs I/O).
    pub bar0_raw: u32,
}

/// Decoded PCI BAR0 from the config-space dword at 0x10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtioPciBar0 {
    /// Memory-mapped base physical address (lower nibble cleared, not mapped by kernel yet).
    Mmio { phys: u32 },
    /// ISA I/O port base (e.g. QEMU legacy virtio-net often uses `0xC000`).
    Io { port_base: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VirtioIoProbe {
    /// `inb(port_base + 18)` after reset from firmware/QEMU.
    LegacyStatus { port_base: u16, status: u8 },
    /// High MMIO BAR or unset BAR — no I/O probe performed.
    SkippedMmioOrUnset,
}

use crate::arch::port::inb;

/// Interpret BAR0 low bit: I/O (`1`) vs 32-bit MMIO (`0`).
pub fn decode_bar0(bar0_raw: u32) -> Option<VirtioPciBar0> {
    if bar0_raw == 0 || bar0_raw == 0xFFFF_FFFF {
        return None;
    }
    if bar0_raw & 1 == 1 {
        let base = (bar0_raw & 0x0000_FFFC) as u16;
        Some(VirtioPciBar0::Io { port_base: base })
    } else {
        let phys = bar0_raw & 0xFFFF_FFF0;
        Some(VirtioPciBar0::Mmio { phys })
    }
}

/// Read legacy virtio device status when BAR0 is an I/O region.
///
/// # Safety
/// `port_base` must be the decoded I/O BAR from this device; caller must not
/// use when BAR is MMIO.
pub unsafe fn legacy_io_read_status(port_base: u16) -> u8 {
    unsafe { inb(port_base.wrapping_add(VIRTIO_PCI_LEGACY_STATUS)) }
}

/// Probe I/O legacy status when possible; otherwise report skipped.
pub fn probe_legacy_io(bar0_raw: u32) -> VirtioIoProbe {
    let Some(decoded) = decode_bar0(bar0_raw) else {
        return VirtioIoProbe::SkippedMmioOrUnset;
    };
    match decoded {
        VirtioPciBar0::Mmio { .. } => VirtioIoProbe::SkippedMmioOrUnset,
        VirtioPciBar0::Io { port_base } => VirtioIoProbe::LegacyStatus {
            port_base,
            status: unsafe { legacy_io_read_status(port_base) },
        },
    }
}

#[inline]
fn is_virtio_net_device(vendor: u16, device: u16) -> bool {
    vendor == VIRTIO_PCI_VENDOR
        && (device == VIRTIO_PCI_DEVICE_NET_LEGACY || device == VIRTIO_PCI_DEVICE_NET_MODERN)
}

/// Scan PCI bus 0, function 0 only, for a virtio-net adapter.
pub fn scan_virtio_net_pci() -> Option<VirtioNetPciInfo> {
    for dev in 0u8..32u8 {
        let func = 0u8;
        let id = unsafe { pci_read32(0, dev, func, 0) };
        let vendor = (id & 0xFFFF) as u16;
        if vendor == 0xFFFF {
            continue;
        }
        let device = ((id >> 16) & 0xFFFF) as u16;
        if !is_virtio_net_device(vendor, device) {
            continue;
        }
        let bar0 = unsafe { pci_read32(0, dev, func, 0x10) };
        return Some(VirtioNetPciInfo {
            bus: 0,
            dev,
            func,
            vendor,
            device,
            bar0_raw: bar0,
        });
    }
    None
}
