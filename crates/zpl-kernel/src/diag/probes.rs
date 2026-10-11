//! The reads the diagnostic is for: ACPI tables, the PCI bus, and the i8042.
//!
//! Every function here fetches bytes and hands them back; turning them into words is
//! `decode`'s job and putting them on the screen is the caller's. The only writes are to
//! the i8042 command port, for the two commands the diagnostic is asked to send, and to
//! the PCI address latch, which selects a register and changes nothing in a device.

use crate::arch::port::{inb, outb};
use crate::drivers::pci::pci_read32;

use super::cpu::{phys_readable, range_mapped, uptime_ms};
use super::decode::{self, Fadt, Rsdp, ST_AUX, ST_INPUT_FULL, ST_OUTPUT_FULL};

// ---------------------------------------------------------------------------
// ACPI
// ---------------------------------------------------------------------------

/// What could be learned from the ACPI tables, and how far the reading got.
pub struct AcpiReport {
    pub rsdp: Option<Rsdp>,
    /// Physical address of the root table that was walked, and whether it was the XSDT.
    pub root: Option<(u64, bool)>,
    pub table_count: usize,
    /// Signatures of the first tables, for the serial log.
    pub signatures: [[u8; 4]; 24],
    pub fadt: Option<Fadt>,
    /// The first thing that could not be read, if anything stopped the walk.
    pub problem: Option<&'static str>,
}

fn copy_from(va: u64, out: &mut [u8]) {
    for (i, slot) in out.iter_mut().enumerate() {
        // SAFETY: every caller has checked `[va, va + out.len())` is mapped, and it is
        // ordinary memory holding firmware tables.
        *slot = unsafe { core::ptr::read_volatile((va + i as u64) as *const u8) };
    }
}

fn read_phys(pa: u64, out: &mut [u8]) -> bool {
    match phys_readable(pa, out.len() as u64) {
        Some(va) => {
            copy_from(va, out);
            true
        }
        None => false,
    }
}

/// Follow the RSDP to the FADT. `rsdp_addr` is what Limine reported, which is a direct-map
/// address in most base revisions and a physical one in revision 3; both are handled.
pub fn read_acpi(rsdp_addr: u64, hhdm: u64) -> AcpiReport {
    let mut r = AcpiReport {
        rsdp: None,
        root: None,
        table_count: 0,
        signatures: [[b' '; 4]; 24],
        fadt: None,
        problem: None,
    };
    let rsdp_va = if rsdp_addr < hhdm { hhdm.wrapping_add(rsdp_addr) } else { rsdp_addr };
    let mut raw = [0u8; decode::RSDP_LEN];
    if !range_mapped(rsdp_va, 20) {
        r.problem = Some("RSDP not mapped");
        return r;
    }
    let long = range_mapped(rsdp_va, decode::RSDP_LEN as u64);
    copy_from(rsdp_va, &mut raw[..if long { decode::RSDP_LEN } else { 20 }]);
    let Some(rsdp) = decode::parse_rsdp(&raw[..if long { decode::RSDP_LEN } else { 20 }]) else {
        r.problem = Some("no RSDP signature");
        return r;
    };
    r.rsdp = Some(rsdp);

    let (root, wide) = if rsdp.xsdt != 0 { (rsdp.xsdt, true) } else { (u64::from(rsdp.rsdt), false) };
    r.root = Some((root, wide));
    let mut header = [0u8; decode::SDT_HEADER_LEN];
    if !read_phys(root, &mut header) {
        r.problem = Some("root table not mapped");
        return r;
    }
    let Some(h) = decode::parse_sdt_header(&header) else { return r };
    let entry_size = if wide { 8 } else { 4 };
    let count = (h.length as usize).saturating_sub(decode::SDT_HEADER_LEN) / entry_size;
    if phys_readable(root, u64::from(h.length)).is_none() {
        r.problem = Some("root table entries not mapped");
        return r;
    }
    for i in 0..count.min(64) {
        let mut e = [0u8; 8];
        let at = root + (decode::SDT_HEADER_LEN + i * entry_size) as u64;
        if !read_phys(at, &mut e[..entry_size]) {
            break;
        }
        let pa = u64::from_le_bytes(e);
        r.table_count += 1;
        let mut th = [0u8; decode::SDT_HEADER_LEN];
        if !read_phys(pa, &mut th) {
            r.problem.get_or_insert("a table is not mapped");
            continue;
        }
        let Some(t) = decode::parse_sdt_header(&th) else { continue };
        if let Some(slot) = r.signatures.get_mut(i) {
            *slot = t.signature;
        }
        if &t.signature == b"FACP" {
            let mut buf = [0u8; 512];
            let n = (t.length as usize).clamp(decode::SDT_HEADER_LEN, buf.len());
            if read_phys(pa, &mut buf[..n]) {
                r.fadt = decode::parse_fadt(&buf[..n]);
            } else {
                r.problem.get_or_insert("FADT body not mapped");
            }
        }
    }
    r
}

// ---------------------------------------------------------------------------
// PCI
// ---------------------------------------------------------------------------

/// One USB host controller, and what it says about who owns it.
#[derive(Clone, Copy)]
pub struct UsbController {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub prog_if: u8,
    pub irq: u8,
    pub legacy: Legacy,
}

/// The legacy-ownership register of a controller, by generation.
#[derive(Clone, Copy)]
pub enum Legacy {
    /// UHCI LEGSUP, PCI config 0xC0.
    Uhci(u16),
    /// EHCI / xHCI USBLEGSUP and USBLEGCTLSTS. `guessed` when the EHCI pointer to it
    /// could not be read and Intel's fixed offset 0x68 was used instead.
    LegSup { legsup: u32, ctlsts: u32, guessed: bool },
    /// OHCI HcControl; bit 8 (InterruptRouting) set means firmware owns it.
    Ohci(u32),
    /// The controller has none, or it could not be found.
    None,
    /// The registers it lives behind are not mapped in this boot.
    Unmapped,
}

pub struct PciReport {
    pub devices: usize,
    pub usb: [Option<UsbController>; 12],
    pub usb_total: usize,
}

fn cfg32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    // SAFETY: a configuration-space read through the legacy ports, ring 0. It latches
    // an address and reads the data window; no device changes.
    unsafe { pci_read32(bus, dev, func, off) }
}

fn mmio32(va: u64) -> u32 {
    // SAFETY: the caller checked the page is mapped; reading these controller registers
    // (capability, legacy support, control) has no side effect.
    unsafe { core::ptr::read_volatile(va as *const u32) }
}

/// Physical base of a memory BAR0, or `None` for an I/O or empty one.
fn mem_bar0(bus: u8, dev: u8, func: u8) -> Option<u64> {
    let lo = cfg32(bus, dev, func, 0x10);
    if lo & 1 != 0 {
        return None;
    }
    let mut base = u64::from(lo & !0xF);
    if (lo >> 1) & 3 == 2 {
        base |= u64::from(cfg32(bus, dev, func, 0x14)) << 32;
    }
    (base != 0).then_some(base)
}

fn mmio_at(bus: u8, dev: u8, func: u8, offset: u64) -> Option<u64> {
    let base = mem_bar0(bus, dev, func)?;
    phys_readable(base + offset, 4)
}

fn legacy_of(bus: u8, dev: u8, func: u8, prog_if: u8, vendor: u16) -> Legacy {
    match prog_if {
        0x00 => Legacy::Uhci((cfg32(bus, dev, func, 0xC0) & 0xFFFF) as u16),
        0x10 => match mmio_at(bus, dev, func, 0x04) {
            Some(va) => Legacy::Ohci(mmio32(va)),
            None => Legacy::Unmapped,
        },
        0x20 => {
            let eecp = mmio_at(bus, dev, func, 0x08).map(|va| ((mmio32(va) >> 8) & 0xFF) as u8);
            let (off, guessed) = match eecp {
                Some(e) if e >= 0x40 => (e, false),
                Some(_) => return Legacy::None,
                None if vendor == 0x8086 => (0x68, true),
                None => return Legacy::Unmapped,
            };
            let legsup = cfg32(bus, dev, func, off);
            if legsup & 0xFF != 1 {
                return if guessed { Legacy::Unmapped } else { Legacy::None };
            }
            Legacy::LegSup { legsup, ctlsts: cfg32(bus, dev, func, off + 4), guessed }
        }
        0x30 => xhci_legacy(bus, dev, func),
        _ => Legacy::None,
    }
}

fn xhci_legacy(bus: u8, dev: u8, func: u8) -> Legacy {
    let Some(base) = mem_bar0(bus, dev, func) else { return Legacy::None };
    let Some(va) = phys_readable(base + 0x10, 4) else { return Legacy::Unmapped };
    let mut offset = u64::from(mmio32(va) >> 16) * 4;
    for _ in 0..64 {
        if offset == 0 {
            break;
        }
        let Some(cap) = phys_readable(base + offset, 8) else { return Legacy::Unmapped };
        let d = mmio32(cap);
        if d & 0xFF == 1 {
            return Legacy::LegSup { legsup: d, ctlsts: mmio32(cap + 4), guessed: false };
        }
        let next = u64::from((d >> 8) & 0xFF) * 4;
        if next == 0 {
            break;
        }
        offset += next;
    }
    Legacy::None
}

/// Every bus, every slot, every function: count devices, keep the USB host controllers.
///
/// A brute-force walk rather than following bridges, so a controller behind a bridge
/// the walk misread is still found. 256 x 32 reads of function 0 takes milliseconds.
pub fn scan_pci() -> PciReport {
    let mut r = PciReport { devices: 0, usb: [None; 12], usb_total: 0 };
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            let id0 = cfg32(bus, dev, 0, 0x00);
            if id0 & 0xFFFF == 0xFFFF || id0 & 0xFFFF == 0 {
                continue;
            }
            let multi = (cfg32(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
            for func in 0..if multi { 8u8 } else { 1u8 } {
                let id = if func == 0 { id0 } else { cfg32(bus, dev, func, 0x00) };
                let vendor = (id & 0xFFFF) as u16;
                if vendor == 0xFFFF || vendor == 0 {
                    continue;
                }
                r.devices += 1;
                let class = cfg32(bus, dev, func, 0x08);
                if class >> 16 != 0x0C03 {
                    continue;
                }
                let prog_if = ((class >> 8) & 0xFF) as u8;
                let found = UsbController {
                    bus,
                    dev,
                    func,
                    vendor,
                    device: (id >> 16) as u16,
                    prog_if,
                    irq: (cfg32(bus, dev, func, 0x3C) & 0xFF) as u8,
                    legacy: legacy_of(bus, dev, func, prog_if, vendor),
                };
                if let Some(slot) = r.usb.get_mut(r.usb_total) {
                    *slot = Some(found);
                }
                r.usb_total += 1;
            }
        }
    }
    r
}

// ---------------------------------------------------------------------------
// i8042
// ---------------------------------------------------------------------------

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;

#[must_use]
pub fn status() -> u8 {
    // SAFETY: the i8042 status port. Reading it consumes nothing.
    unsafe { inb(STATUS) }
}

/// Take the waiting byte from 0x60. Only call with the output-full bit seen set.
#[must_use]
pub fn data() -> u8 {
    // SAFETY: the i8042 data port, read because the status said a byte is waiting; the
    // diagnostic is the only reader in this image.
    unsafe { inb(DATA) }
}

fn wait_input_empty(ms: u64) -> bool {
    let until = uptime_ms() + ms;
    loop {
        if status() & ST_INPUT_FULL == 0 {
            return true;
        }
        if uptime_ms() > until {
            return false;
        }
    }
}

/// A keyboard-side byte, if one arrives within `ms`. Bytes from the auxiliary device are
/// read and passed to `aux` so they are counted rather than lost.
fn wait_output(ms: u64, aux: &mut dyn FnMut(u8)) -> Option<u8> {
    let until = uptime_ms() + ms;
    loop {
        let s = status();
        if s != 0xFF && s & ST_OUTPUT_FULL != 0 {
            let b = data();
            if s & ST_AUX == 0 {
                return Some(b);
            }
            aux(b);
        }
        if uptime_ms() > until {
            return None;
        }
    }
}

/// Why an i8042 command got no answer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CmdError {
    /// The controller never took the command (input buffer stayed full).
    NotAccepted,
    /// It took the command and said nothing back in time.
    NoReply,
}

/// Send a controller command and read its one-byte reply.
pub fn command_read(cmd: u8, timeout_ms: u64, aux: &mut dyn FnMut(u8)) -> Result<u8, CmdError> {
    if !wait_input_empty(50) {
        return Err(CmdError::NotAccepted);
    }
    // SAFETY: the i8042 command port; the command is one of the two this diagnostic is
    // asked to send (0x20 read config, 0xAA self-test).
    unsafe { outb(STATUS, cmd) };
    wait_output(timeout_ms, aux).ok_or(CmdError::NoReply)
}

/// Write the configuration byte back (command 0x60), used only to undo a self-test
/// that changed it.
pub fn write_config(value: u8) -> bool {
    if !wait_input_empty(50) {
        return false;
    }
    // SAFETY: command 0x60 then its data byte, restoring the value the firmware left.
    unsafe { outb(STATUS, 0x60) };
    if !wait_input_empty(50) {
        return false;
    }
    // SAFETY: as above.
    unsafe { outb(DATA, value) };
    true
}
