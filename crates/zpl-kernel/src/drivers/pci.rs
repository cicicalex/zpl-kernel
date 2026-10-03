//! PCI bus enumeration over the legacy configuration ports.
//!
//! Before this, the kernel looked at the PCI bus exactly once and only for one
//! thing: `virtio_net::scan_virtio_net_pci` walked bus 0 asking "is this the
//! virtio network card?" and reported nothing when it was not. So a boot on any
//! machine said the same sentence about the hardware — that one device is
//! absent — and nothing about what is actually there.
//!
//! This walks the whole tree and names what it finds. It is the smallest piece
//! of real hardware interrogation the kernel can do: every value printed was
//! read from a device, so the output differs between machines, which is the
//! point.
//!
//! # How the walk works
//!
//! Configuration space is reached through two I/O ports — an address latch at
//! `0xCF8` and a data window at `0xCFC`. A device is identified by
//! (bus, device, function); a vendor ID of `0xFFFF` means nothing is there.
//!
//! Only function 0 is probed unless the header type says the device is
//! multi-function (bit 7), because probing absent functions on some real
//! hardware returns rubbish rather than `0xFFFF`.
//!
//! Buses beyond 0 are reached through PCI-to-PCI bridges: a bridge's
//! configuration header carries the number of the bus behind it, and that bus is
//! queued rather than recursed into — an explicit queue keeps the kernel stack
//! out of it, and a cycle in a malformed topology cannot run away.
//!
//! # What this does not do
//!
//! It does not program base address registers, enable bus mastering, or touch a
//! device in any way. Every access here is a read. Making a device *work* is
//! the next step; this is the one that says which devices are present.

// The classification and the walk's bookkeeping are ordinary data handling and
// are compiled everywhere, so the tests at the bottom of this file run on a
// development machine. Only the parts that touch the configuration ports are
// bare-metal, and they are marked one by one.

/// Configuration address latch.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
/// Configuration data window.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// # Safety
/// Ring 0, and `port` must be one of the two PCI configuration ports.
/// Writing anywhere else can change device state.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline]
unsafe fn outl(port: u16, val: u32) {
    // SAFETY: the caller's contract -- ring 0, and `port` is one of the two
    // configuration ports, which have no side effect beyond latching an address.
    unsafe { crate::arch::port::outl(port, val) }
}

/// # Safety
/// Ring 0, and `port` must be one of the two PCI configuration ports.
/// Reading elsewhere can have side effects.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline]
unsafe fn inl(port: u16) -> u32 {
    // SAFETY: as above; reading the data window has no effect on any device.
    unsafe { crate::arch::port::inl(port) }
}

/// Read a dword from configuration space.
///
/// # Safety
/// Ring 0 on a machine with the legacy configuration ports. This is a read: it
/// latches an address and reads the window, and changes nothing in any device.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) unsafe fn pci_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((u32::from(dev)) << 11)
        | ((u32::from(func)) << 8)
        | (u32::from(offset & 0xFC));
    // SAFETY: the caller's contract, and `addr` has bit 31 set as the hardware
    // requires for a configuration cycle.
    unsafe {
        outl(PCI_CONFIG_ADDRESS, addr);
        inl(PCI_CONFIG_DATA)
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Buses that may be queued for a walk. A machine with more PCI-to-PCI bridges
/// than this exists, but not one this kernel runs on; the count is reported so a
/// truncated walk says so rather than looking complete.
const MAX_BUSES: usize = 16;

/// Devices reported in full before the walk switches to counting only. Bounds
/// both the time spent in early boot and the number of lines on a 25-row screen.
const MAX_REPORTED: usize = 24;

/// One device, as read from its configuration header.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    /// Raw BAR0 dword. Bit 0 tells I/O from memory; the rest is the base once
    /// the low flag bits are cleared. Reported raw so nothing is lost.
    pub bar0_raw: u32,
    /// Interrupt line from configuration offset 0x3C. `0xFF` means "not
    /// connected", which is what QEMU reports for devices it does not wire up.
    pub interrupt_line: u8,
}

impl PciDevice {
    /// True when this is a PCI-to-PCI bridge, whose secondary bus needs walking.
    #[must_use]
    pub fn is_pci_bridge(&self) -> bool {
        self.class == 0x06 && self.subclass == 0x04
    }
}

/// What the walk found.
pub struct PciScan {
    pub devices: [Option<PciDevice>; MAX_REPORTED],
    /// Devices found, including any past `MAX_REPORTED` that were not recorded.
    pub total: usize,
    pub buses_walked: usize,
    /// True when a bridge pointed at a bus the queue had no room for.
    pub truncated: bool,
}

/// Write a dword into configuration space.
///
/// The counterpart of [`pci_read32`], and the first thing in this kernel that
/// changes a device rather than looking at one. Kept next to the read so the
/// asymmetry is visible: everything else in this module is a read, and this is
/// not.
///
/// # Safety
/// Ring 0 on a machine with the legacy configuration ports, and the caller must
/// know what the register at `offset` does on the device at (bus, dev, func).
/// A wrong write to configuration space can move a device's address windows on
/// top of something else, or switch off the decoding the firmware set up.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub(crate) unsafe fn pci_write32(bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((u32::from(dev)) << 11)
        | ((u32::from(func)) << 8)
        | (u32::from(offset & 0xFC));
    // SAFETY: the caller's contract. Bit 31 of `addr` selects a configuration
    // cycle, as the hardware requires.
    unsafe {
        outl(PCI_CONFIG_ADDRESS, addr);
        outl(PCI_CONFIG_DATA, value);
    }
}

/// Configuration offset of the command register.
pub const REG_COMMAND: u8 = 0x04;
/// Command bit 2: the device may act as a bus master, i.e. start its own DMA.
/// Nothing a device does to memory on its own works without this.
pub const COMMAND_BUS_MASTER: u32 = 1 << 2;
/// Command bit 1: the device answers memory cycles at its memory BARs.
pub const COMMAND_MEMORY_SPACE: u32 = 1 << 1;

/// The base of BAR0 when it is a memory BAR, joining BAR1 in when BAR0 says it is
/// 64-bit wide (bits 2:1 = `10`). `bar1` is only looked at in that case.
#[must_use]
pub fn memory_bar0_base(bar0: u32, bar1: u32) -> Option<u64> {
    if bar0 & 1 != 0 {
        return None; // an I/O BAR
    }
    let low = u64::from(bar0 & 0xFFFF_FFF0);
    let base = if bar0 & 0b110 == 0b100 {
        low | (u64::from(bar1) << 32)
    } else {
        low
    };
    (base != 0).then_some(base)
}

/// Read BAR0 (and BAR1 with it, for a 64-bit BAR) and return the memory base.
///
/// # Safety
/// Ring 0 on a machine with the legacy configuration ports; reads only.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
// Its one caller, the USB keyboard driver, is built only with the prompt.
#[cfg_attr(not(feature = "shell"), allow(dead_code))]
pub(crate) unsafe fn read_memory_bar0(dev: &PciDevice) -> Option<u64> {
    // SAFETY: the caller's contract; offset 0x14 is BAR1 in a type-0 header, and is
    // only used when BAR0 says the two belong together.
    let bar1 = unsafe { pci_read32(dev.bus, dev.device, dev.function, 0x14) };
    memory_bar0_base(dev.bar0_raw, bar1)
}

/// Let a device answer at its memory BARs and start its own DMA.
///
/// Read-modify-write of the command register: every bit the firmware set stays, and
/// only memory-space decoding and bus mastering are forced on.
///
/// # Safety
/// `dev` must be a device found by [`scan`], and the caller must be its driver: a
/// device with bus mastering on may write anywhere in memory it has been pointed at.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
// Its one caller, the USB keyboard driver, is built only with the prompt.
#[cfg_attr(not(feature = "shell"), allow(dead_code))]
pub(crate) unsafe fn enable_memory_and_bus_master(dev: &PciDevice) {
    // SAFETY: the caller's contract; offset 0x04 is the command register.
    unsafe {
        let cmd = pci_read32(dev.bus, dev.device, dev.function, REG_COMMAND);
        pci_write32(
            dev.bus,
            dev.device,
            dev.function,
            REG_COMMAND,
            cmd | COMMAND_MEMORY_SPACE | COMMAND_BUS_MASTER,
        );
    }
}

/// True for a USB xHCI controller: class 0x0C (serial bus), subclass 0x03 (USB),
/// programming interface 0x30 (xHCI).
#[must_use]
pub fn is_xhci(dev: &PciDevice) -> bool {
    dev.class == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30
}

/// A short human name for a class/subclass pair.
///
/// Deliberately small: the pairs QEMU presents on its default machines plus the
/// ones the drivers in this tree care about. Anything else prints its numbers,
/// which is more honest than a wrong guess.
#[must_use]
pub fn class_name(class: u8, subclass: u8, prog_if: u8) -> &'static [u8] {
    match (class, subclass) {
        (0x00, _) => b"legacy (pre-class-code)",
        (0x01, 0x01) => b"IDE controller",
        (0x01, 0x06) => b"SATA controller",
        (0x01, 0x08) => b"NVMe controller",
        (0x01, _) => b"mass storage",
        (0x02, 0x00) => b"ethernet controller",
        (0x02, _) => b"network controller",
        (0x03, 0x00) => b"VGA display",
        (0x03, _) => b"display controller",
        (0x04, 0x01) => b"audio device",
        (0x04, 0x03) => b"HD audio",
        (0x04, _) => b"multimedia",
        (0x06, 0x00) => b"host bridge",
        (0x06, 0x01) => b"ISA bridge",
        (0x06, 0x04) => b"PCI-to-PCI bridge",
        (0x06, _) => b"bridge",
        (0x0C, 0x03) => match prog_if {
            0x00 => b"USB UHCI",
            0x10 => b"USB OHCI",
            0x20 => b"USB EHCI",
            0x30 => b"USB xHCI",
            _ => b"USB controller",
        },
        (0x0C, 0x05) => b"SMBus",
        (0x0C, _) => b"serial bus",
        (0xFF, _) => b"unassigned",
        _ => b"unknown",
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Read one device's header, or `None` when the slot is empty.
///
/// # Safety
/// Ring 0 on a machine with the legacy PCI configuration ports. Every access is
/// a read of configuration space; nothing here writes to a device.
unsafe fn read_header(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    // SAFETY: the caller's contract. `pci_read32` only latches an address and
    // reads the data window; it has no effect on the device.
    let id = unsafe { pci_read32(bus, device, function, 0x00) };
    let vendor = (id & 0xFFFF) as u16;
    if vendor == 0xFFFF || vendor == 0x0000 {
        return None;
    }
    // SAFETY: as above. Offset 0x08 carries revision, prog-if, subclass, class;
    // offset 0x0C carries the header type in its third byte.
    let class_dword = unsafe { pci_read32(bus, device, function, 0x08) };
    let header_dword = unsafe { pci_read32(bus, device, function, 0x0C) };
    // SAFETY: as above. Offset 0x10 is BAR0 for both header types 0 and 1;
    // offset 0x3C carries the interrupt line in its low byte.
    let bar0_raw = unsafe { pci_read32(bus, device, function, 0x10) };
    let irq_dword = unsafe { pci_read32(bus, device, function, 0x3C) };
    Some(PciDevice {
        bus,
        device,
        function,
        vendor,
        device_id: ((id >> 16) & 0xFFFF) as u16,
        class: ((class_dword >> 24) & 0xFF) as u8,
        subclass: ((class_dword >> 16) & 0xFF) as u8,
        prog_if: ((class_dword >> 8) & 0xFF) as u8,
        header_type: ((header_dword >> 16) & 0xFF) as u8,
        bar0_raw,
        interrupt_line: (irq_dword & 0xFF) as u8,
    })
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// The bus behind a PCI-to-PCI bridge, from configuration offset 0x19.
///
/// # Safety
/// Same contract as [`read_header`], and the device must be a bridge.
unsafe fn secondary_bus(dev: &PciDevice) -> u8 {
    // SAFETY: the caller's contract. Offset 0x18 holds primary, secondary and
    // subordinate bus numbers in its low three bytes; the second is the one
    // devices behind the bridge answer on.
    let dword = unsafe { pci_read32(dev.bus, dev.device, dev.function, 0x18) };
    ((dword >> 8) & 0xFF) as u8
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Walk the tree from bus 0 and report what is on it.
///
/// Reads only. Safe to call before paging is set up and before interrupts are
/// enabled, which is why the boot calls it where it does.
#[must_use]
pub fn scan() -> PciScan {
    let mut out = PciScan {
        devices: [None; MAX_REPORTED],
        total: 0,
        buses_walked: 0,
        truncated: false,
    };

    let mut queue = [0u8; MAX_BUSES];
    let mut queued = 1usize; // bus 0 is always there
    let mut seen = [false; 256];
    seen[0] = true;
    let mut head = 0usize;

    while head < queued {
        let bus = queue[head];
        head += 1;
        out.buses_walked += 1;

        for device in 0u8..32 {
            // SAFETY: ring 0, configuration-space read only. An absent slot
            // answers 0xFFFF and is skipped.
            let Some(first) = (unsafe { read_header(bus, device, 0) }) else {
                continue;
            };
            // Bit 7 of the header type marks a multi-function device. Probing
            // functions 1..8 of a single-function device is not merely wasteful:
            // some hardware aliases them onto function 0, which would report the
            // same device eight times.
            let functions = if first.header_type & 0x80 != 0 { 8u8 } else { 1u8 };

            for function in 0..functions {
                let found = if function == 0 {
                    Some(first)
                } else {
                    // SAFETY: as above.
                    unsafe { read_header(bus, device, function) }
                };
                let Some(dev) = found else { continue };

                if out.total < MAX_REPORTED {
                    out.devices[out.total] = Some(dev);
                }
                out.total += 1;

                if dev.is_pci_bridge() {
                    // SAFETY: as above, and `is_pci_bridge` is true.
                    let behind = unsafe { secondary_bus(&dev) };
                    if !seen[behind as usize] {
                        if queued < MAX_BUSES {
                            seen[behind as usize] = true;
                            queue[queued] = behind;
                            queued += 1;
                        } else {
                            out.truncated = true;
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pci_to_pci_bridge_is_recognised_by_class_and_subclass() {
        let bridge = PciDevice {
            bus: 0, device: 1, function: 0, vendor: 0x8086, device_id: 0x244E,
            class: 0x06, subclass: 0x04, prog_if: 0x00, header_type: 0x01,
            bar0_raw: 0, interrupt_line: 0xFF,
        };
        assert!(bridge.is_pci_bridge());
    }

    #[test]
    fn a_host_bridge_is_not_a_pci_to_pci_bridge() {
        // Both are class 6. Walking into a host bridge's "secondary bus" would
        // read a field that does not exist in its header.
        let host = PciDevice {
            bus: 0, device: 0, function: 0, vendor: 0x8086, device_id: 0x1237,
            class: 0x06, subclass: 0x00, prog_if: 0x00, header_type: 0x00,
            bar0_raw: 0, interrupt_line: 0xFF,
        };
        assert!(!host.is_pci_bridge());
    }

    #[test]
    fn usb_controllers_are_told_apart_by_prog_if() {
        assert_eq!(class_name(0x0C, 0x03, 0x30), b"USB xHCI");
        assert_eq!(class_name(0x0C, 0x03, 0x20), b"USB EHCI");
        assert_eq!(class_name(0x0C, 0x03, 0x00), b"USB UHCI");
    }

    #[test]
    fn a_64_bit_bar_joins_its_upper_half() {
        // Memory, 64-bit (bits 2:1 = 10), prefetchable.
        assert_eq!(memory_bar0_base(0xFEB0_000C, 0x0000_0001), Some(0x1_FEB0_0000));
        // 32-bit: BAR1 belongs to something else and is ignored.
        assert_eq!(memory_bar0_base(0xFEB0_0000, 0xDEAD_BEEF), Some(0xFEB0_0000));
        // I/O, and unset.
        assert_eq!(memory_bar0_base(0x0000_C001, 0), None);
        assert_eq!(memory_bar0_base(0x0000_0004, 0), None);
    }

    #[test]
    fn only_the_xhci_programming_interface_is_xhci() {
        let mut d = PciDevice {
            bus: 0, device: 4, function: 0, vendor: 0x1B36, device_id: 0x000D,
            class: 0x0C, subclass: 0x03, prog_if: 0x30, header_type: 0x00,
            bar0_raw: 0xFEB0_0004, interrupt_line: 0xFF,
        };
        assert!(is_xhci(&d));
        d.prog_if = 0x20; // EHCI
        assert!(!is_xhci(&d));
    }

    #[test]
    fn an_unknown_pair_says_so_instead_of_guessing() {
        assert_eq!(class_name(0x42, 0x42, 0x00), b"unknown");
    }

    #[test]
    fn the_classes_the_driver_plan_cares_about_have_names() {
        for (c, s, p) in [(0x02, 0x00, 0x00), (0x03, 0x00, 0x00),
                          (0x04, 0x03, 0x00), (0x0C, 0x03, 0x30)] {
            assert_ne!(class_name(c, s, p), b"unknown", "class {c:#x}:{s:#x}");
        }
    }
}
