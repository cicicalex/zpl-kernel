//! Intel e1000 (82540EM) — far enough to read the card's own MAC address.
//!
//! QEMU puts one of these in its default machine, so this needs no extra
//! `-device`: the enumeration in [`crate::drivers::pci`] finds it at `00:03.0`
//! with a memory BAR at `0xfebc0000` and IRQ 11.
//!
//! # Why this step and not a working driver
//!
//! Everything the kernel had read from hardware until now came from PCI
//! *configuration* space — a side channel every device answers on, through two
//! I/O ports, with no mapping involved. This reads the device's **own
//! registers**, which means mapping its BAR into the page tables and touching
//! memory that is not memory. It is the first time the kernel looks at a device
//! rather than at the bus's description of it.
//!
//! The MAC address is the right thing to read first because it is
//! *checkable from outside*: QEMU hands its cards addresses beginning `52:54:00`,
//! so a plausible-looking wrong answer is still recognisably wrong.
//!
//! # What this does not do
//!
//! Reads only. It does not enable bus mastering, set up descriptor rings, unmask
//! IRQ 11, or send anything. Those are the later steps of driver bring-up,
//! and the first of them is the first write this kernel would ever make to a
//! device.

// As in `pci`, the classification and decoding build everywhere so they can be
// tested on a development machine; only the parts that map and read device
// memory are bare-metal, and they are gated one by one.

use crate::drivers::pci::PciDevice;

/// Intel.
pub const E1000_VENDOR: u16 = 0x8086;
/// 82540EM, the model QEMU emulates as `e1000`.
pub const E1000_DEVICE_82540EM: u16 = 0x100E;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Receive Address Low — the first four bytes of the MAC, plus validity in RAH.
const REG_RAL0: u64 = 0x5400;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Receive Address High — the last two bytes, and bit 31 says the entry is valid.
const REG_RAH0: u64 = 0x5404;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Device Status. Bit 1 is "link up", bit 0 is "full duplex".
const REG_STATUS: u64 = 0x0008;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// MDI Control: the door to the PHY, one register at a time. Write what you
/// want, poll bit 28 until the card says the answer is there, read it back out
/// of the low sixteen bits.
const REG_MDIC: u64 = 0x0020;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// The PHY answers at address 1 on this card.
const PHY_ADDRESS: u32 = 1;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// PHY register 1, the MII status register. Bit 2 is link, bit 5 is
/// "auto-negotiation finished".
const PHY_REG_STATUS: u32 = 1;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const MDIC_OP_READ: u32 = 0b10 << 26;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const MDIC_READY: u32 = 1 << 28;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const MDIC_ERROR: u32 = 1 << 30;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// MII status bit 2: the PHY says the link is up.
const MII_LINK_UP: u16 = 1 << 2;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// MII status bit 5: auto-negotiation has finished.
const MII_AUTONEG_DONE: u16 = 1 << 5;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Interrupt Cause Read. Reading it reports what the card wants attention for
/// and clears those causes in the same access, which is why the handler must
/// keep what it read.
const REG_ICR: u64 = 0x00C0;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Interrupt Mask Set: a one here lets that cause raise the line.
const REG_IMS: u64 = 0x00D0;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Receive timer expired -- a frame is in the ring.
const ICR_RXT0: u32 = 1 << 7;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Receive descriptor minimum threshold: the ring is running low.
const ICR_RXDMT0: u32 = 1 << 4;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Set in RAH when the address entry holds a real address.
const RAH_ADDRESS_VALID: u32 = 1 << 31;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Where the BAR is mapped. Inside the region `paging` allows, above everything
/// the ring-3 demo and the stack guard use, which occupy the first 128 KiB.
const MMIO_VIRT_BASE: u64 = crate::mm::paging::USER_REGION_BASE + 0x10_0000;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const MMIO_PAGE_SIZE: u64 = 4096;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// How many pages of the register block to map.
///
/// The first version of this mapped **one** page, on the reasoning that the
/// registers being read "live in the first 32 KiB" -- true, and irrelevant,
/// because one page is 4 KiB and `RAL0` is at `0x5400`. The read went past the
/// mapping, the page-fault handler installed a placeholder page and returned,
/// and the card reported a MAC address of all zeroes with the valid bit clear.
/// Nothing crashed and nothing complained; a handler that recovers silently is
/// a handler that hides the mistake it recovered from.
///
/// Eight pages cover `0x0000..0x7FFF`, and the assertion below makes the class
/// of mistake a build error instead of a plausible-looking zero.
const MMIO_PAGES: u64 = 8;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const fn max2(a: u64, b: u64) -> u64 {
    if a > b { a } else { b }
}

/// Every register this module touches has to be inside the mapped window; this
/// is the highest end offset among them, so adding one above the window becomes
/// a build error rather than a silent read of a placeholder page.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const HIGHEST_REG_END: u64 = max2(
    REG_RAH0 + 4,
    max2(
        max2(REG_TDT + 4, max2(REG_TCTL + 4, REG_TIPG + 4)),
        max2(max2(REG_RDT + 4, REG_RCTL + 4), max2(REG_ICR + 4, REG_IMS + 4)),
    ),
);
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const _: () = assert!(
    HIGHEST_REG_END <= MMIO_PAGES * MMIO_PAGE_SIZE,
    "a register is read from outside the mapped window; map more pages"
);

/// A MAC address, and whether the card said it was valid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MacAddress {
    pub bytes: [u8; 6],
    /// RAH bit 31. A card that has not been given an address clears this, and
    /// then the six bytes mean nothing.
    pub valid: bool,
}

impl MacAddress {
    /// True for the `52:54:00` prefix QEMU gives its emulated cards.
    ///
    /// Not a correctness check — a real card has a manufacturer's prefix — but
    /// it is what makes the first read verifiable under QEMU rather than merely
    /// plausible.
    #[must_use]
    pub fn looks_like_qemu(&self) -> bool {
        self.bytes[0] == 0x52 && self.bytes[1] == 0x54 && self.bytes[2] == 0x00
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum E1000Error {
    /// The BAR was zero, unset, or an I/O BAR rather than a memory one.
    NoMemoryBar,
    /// The BAR base is not page-aligned, so a 4 KiB mapping cannot cover it.
    BarMisaligned,
    /// `paging::map_4k_mmio_page` refused the mapping.
    MapFailed,
}

/// Is this the card we know how to talk to?
#[must_use]
pub fn is_e1000(dev: &PciDevice) -> bool {
    dev.vendor == E1000_VENDOR && dev.device_id == E1000_DEVICE_82540EM
}

/// The 32-bit memory base a BAR reports, if it is a memory BAR at all.
///
/// Bit 0 clears for memory; bits 2:1 give the width (`00` is 32-bit, `10` is
/// 64-bit); bit 3 is prefetchable. The address is what is left once those four
/// are masked off.
#[must_use]
pub fn memory_bar_base(bar_raw: u32) -> Option<u32> {
    if bar_raw == 0 || bar_raw == 0xFFFF_FFFF {
        return None;
    }
    if bar_raw & 1 != 0 {
        return None; // an I/O BAR; this card should not have one here
    }
    Some(bar_raw & 0xFFFF_FFF0)
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Read a 32-bit register at `offset` from the mapped window.
///
/// # Safety
/// The BAR must already be mapped at [`MMIO_VIRT_BASE`], and `offset` must be
/// inside the mapped window -- see `MMIO_PAGES`.
unsafe fn read_reg(offset: u64) -> u32 {
    let addr = (MMIO_VIRT_BASE + offset) as *const u32;
    // SAFETY: the caller's contract. `read_volatile` is required rather than a
    // plain read: this is a device register, and the compiler must not cache it,
    // reorder it or elide it.
    unsafe { core::ptr::read_volatile(addr) }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// Map the card's registers and read its MAC address.
///
/// # Safety
/// `dev` must be an e1000 found by the PCI enumeration, so its BAR describes
/// that card's registers and nothing else. [`MMIO_VIRT_BASE`] must not be in use
/// by anything else, which the constant's own comment explains.
pub unsafe fn read_mac(dev: &PciDevice) -> Result<MacAddress, E1000Error> {
    let base = memory_bar_base(dev.bar0_raw).ok_or(E1000Error::NoMemoryBar)?;
    if u64::from(base) % MMIO_PAGE_SIZE != 0 {
        return Err(E1000Error::BarMisaligned);
    }

    // SAFETY: `base` came from this device's BAR, so it is device memory and not
    // RAM, which is exactly what `map_4k_mmio_page` requires and the opposite of
    // what the ordinary mapper requires. The virtual pages are the kernel's own
    // choice and are used by nothing else.
    for page in 0..MMIO_PAGES {
        let offset = page * MMIO_PAGE_SIZE;
        unsafe {
            crate::mm::paging::map_4k_mmio_page(
                MMIO_VIRT_BASE + offset,
                u64::from(base) + offset,
            )
            .map_err(|_| E1000Error::MapFailed)?;
        }
    }

    // SAFETY: the mapping above succeeded, and both offsets are inside the first
    // page of the register block.
    let (ral, rah) = unsafe { (read_reg(REG_RAL0), read_reg(REG_RAH0)) };

    Ok(MacAddress {
        bytes: [
            (ral & 0xFF) as u8,
            ((ral >> 8) & 0xFF) as u8,
            ((ral >> 16) & 0xFF) as u8,
            ((ral >> 24) & 0xFF) as u8,
            (rah & 0xFF) as u8,
            ((rah >> 8) & 0xFF) as u8,
        ],
        valid: rah & RAH_ADDRESS_VALID != 0,
    })
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
/// The device status register, read from the window [`read_mac`] mapped.
///
/// # Safety
/// [`read_mac`] must have succeeded first, so the mapping exists.
pub unsafe fn read_status() -> u32 {
    // SAFETY: the caller's contract; the offset is in the first page.
    unsafe { read_reg(REG_STATUS) }
}

/// Bit 1 of the status register.
#[must_use]
pub fn status_link_up(status: u32) -> bool {
    status & (1 << 1) != 0
}

// ---------------------------------------------------------------- transmit

/// Transmit Control. Bit 1 enables the transmitter; bit 3 pads short frames.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TCTL: u64 = 0x0400;
/// Transmit Inter-Packet Gap. The card will not transmit with this at zero.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TIPG: u64 = 0x0410;
/// Transmit Descriptor Base Address, low and high halves.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TDBAL: u64 = 0x3800;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TDBAH: u64 = 0x3804;
/// Transmit Descriptor ring Length, in bytes. Must be a multiple of 128.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TDLEN: u64 = 0x3808;
/// Transmit Descriptor Head -- the card's cursor. Read-only to us in practice.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TDH: u64 = 0x3810;
/// Transmit Descriptor Tail -- our cursor. Writing it is what says "go".
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_TDT: u64 = 0x3818;

/// TCTL.EN -- transmitter enabled.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TCTL_EN: u32 = 1 << 1;
/// TCTL.PSP -- pad short packets to the 60-byte Ethernet minimum.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TCTL_PSP: u32 = 1 << 3;
/// Collision threshold, in bits 11:4. The manual's suggested value is 15.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TCTL_CT_15: u32 = 15 << 4;
/// Collision distance, bits 21:12. 64 for full duplex.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TCTL_COLD_64: u32 = 64 << 12;
/// The IPG value the manual gives for copper at 1 Gb/s.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TIPG_DEFAULT: u32 = 0x0060_200A;

/// Descriptor command: this is the last descriptor of the packet.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TXD_CMD_EOP: u8 = 1 << 0;
/// Descriptor command: the card appends the Ethernet checksum.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TXD_CMD_IFCS: u8 = 1 << 1;
/// Descriptor command: report status back into the descriptor when done.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TXD_CMD_RS: u8 = 1 << 3;
/// Descriptor status: descriptor done. The card sets this; we poll it.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TXD_STA_DD: u8 = 1 << 0;

/// Descriptors in the ring. One would do for a single frame; sixteen keeps the
/// ring length a multiple of 128 bytes, which the card requires.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TX_RING_LEN: usize = 16;
/// A legacy transmit descriptor is sixteen bytes.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const TXD_SIZE: usize = 16;

/// Everything the transmit path needs, once.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub struct TxRing {
    /// Physical address of the descriptor ring. The card reads this itself, so
    /// it must be a physical address and it must stay put.
    ring_phys: u64,
    /// Physical address of the one packet buffer.
    buffer_phys: u64,
    /// Which descriptor the next frame goes in.
    ///
    /// Without this every send used descriptor 0 and set the tail to 1, so the
    /// first frame went out and the second was never looked at: the tail was
    /// already where it was being set to, which the card reads as "nothing new".
    /// It reported `tx not confirmed by the card FAIL`, which is the right
    /// complaint about the wrong thing -- the card was fine, it had not been
    /// asked. Found by sending a second frame from the console; the boot path
    /// only ever sent one, so nothing before this had tried.
    next: core::sync::atomic::AtomicU32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TxError {
    /// The frame allocator had nothing left.
    NoFrame,
    /// A frame came back from above the identity map, where the kernel cannot
    /// write to it through its physical address.
    FrameAboveIdentityMap,
    /// The frame did not fit in the buffer.
    FrameTooLong,
    /// The card did not set the descriptor-done bit within the poll budget.
    NotSentInTime,
}

/// How far up physical memory the kernel's identity map reaches.
///
/// Below this, a physical address and a virtual address are the same number, so
/// a DMA buffer can be filled through its physical address directly. A frame
/// from above it would need its own mapping, and this code does not make one --
/// so it refuses rather than writing somewhere it has not mapped.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const IDENTITY_MAP_TOP: u64 = 1 << 30; // 1 GiB

/// The address the CPU uses to reach a frame the card also reads.
///
/// **Two different addresses for the same memory, and the difference matters.**
/// The card walks no page tables: every address handed to it through a register or
/// written into a descriptor must be *physical*. The CPU, on the Limine boot path,
/// cannot use those addresses directly -- it runs in the higher half and reaches
/// physical memory through the HHDM window. On the multiboot path, where low memory
/// is identity-mapped, the two are the same number, which is why this was invisible
/// until the driver was pointed at the other path.
///
/// So: this function for the CPU's own reads and writes, and the bare physical
/// address for anything the card will read.
///
/// Measured, not assumed: with the arm lifted onto the Limine path, the marker before
/// zeroing the descriptor ring printed and the one after it never did.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline]
fn cpu_addr(phys: u64) -> u64 {
    crate::mm::phys_hhdm::pa_to_kernel_va(phys)
}

/// Whether a frame at `phys` is reachable by the CPU at all.
///
/// With an HHDM window every physical address is reachable, so the 1 GiB limit is
/// about the identity-mapped world only. Keeping the check unconditional would refuse
/// perfectly good frames on the path that has the window.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[inline]
fn cpu_can_reach(phys: u64) -> bool {
    crate::mm::phys_hhdm::current_hhdm_offset().is_some() || phys < IDENTITY_MAP_TOP
}

/// Write a 32-bit register in the mapped window.
///
/// # Safety
/// [`read_mac`] must have mapped the window, and `offset` must be inside it.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
unsafe fn write_reg(offset: u64, value: u32) {
    let addr = (MMIO_VIRT_BASE + offset) as *mut u32;
    // SAFETY: the caller's contract. `write_volatile` because this is a device
    // register: the compiler must not merge, reorder or drop the store.
    unsafe { core::ptr::write_volatile(addr, value) }
}

/// Turn on bus mastering so the card may fetch descriptors and packet data.
///
/// **This is the first write this kernel makes to a device.** Everything before
/// it -- the enumeration, the MAC address, the link status -- was a read. A card
/// without this bit ignores the descriptor ring entirely: the registers accept
/// the addresses and nothing ever happens, which is a confusing way to fail.
///
/// # Safety
/// `dev` must be the e1000 the enumeration found; writing the command register
/// of a different device would change that device's decoding.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn enable_bus_master(dev: &PciDevice) {
    use crate::drivers::pci;
    // SAFETY: the caller's contract. Read-modify-write of the command register
    // keeps every bit the firmware set, including memory-space decoding; only
    // the two bits named are forced on.
    unsafe {
        let cmd = pci::pci_read32(dev.bus, dev.device, dev.function, pci::REG_COMMAND);
        pci::pci_write32(
            dev.bus,
            dev.device,
            dev.function,
            pci::REG_COMMAND,
            cmd | pci::COMMAND_BUS_MASTER | pci::COMMAND_MEMORY_SPACE,
        );
    }
}

/// Allocate the ring and the buffer, and point the card at them.
///
/// # Safety
/// [`read_mac`] must have mapped the register window, and bus mastering must be
/// on -- otherwise the card will not read the ring this points it at.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn tx_init() -> Result<TxRing, TxError> {
    let ring_phys = crate::mm::frame_alloc::alloc_frame().ok_or(TxError::NoFrame)?;
    let buffer_phys = crate::mm::frame_alloc::alloc_frame().ok_or(TxError::NoFrame)?;
    if !cpu_can_reach(ring_phys) || !cpu_can_reach(buffer_phys) {
        return Err(TxError::FrameAboveIdentityMap);
    }

    // SAFETY: both frames came from the allocator, so the kernel owns them, and
    // `cpu_addr` gives the address this boot path can write through. The ring is
    // zeroed because the card reads every descriptor's status byte, and a stale one
    // would look like a finished send.
    unsafe {
        core::ptr::write_bytes(cpu_addr(ring_phys) as *mut u8, 0, TX_RING_LEN * TXD_SIZE);
    }

    let ring_bytes = (TX_RING_LEN * TXD_SIZE) as u32;
    // SAFETY: the window is mapped by the caller's contract and every offset
    // below is inside it -- see the assertion next to `MMIO_PAGES`.
    unsafe {
        write_reg(REG_TDBAL, (ring_phys & 0xFFFF_FFFF) as u32);
        write_reg(REG_TDBAH, (ring_phys >> 32) as u32);
        write_reg(REG_TDLEN, ring_bytes);
        write_reg(REG_TDH, 0);
        write_reg(REG_TDT, 0);
        write_reg(REG_TIPG, TIPG_DEFAULT);
        write_reg(REG_TCTL, TCTL_EN | TCTL_PSP | TCTL_CT_15 | TCTL_COLD_64);
    }

    Ok(TxRing { ring_phys, buffer_phys, next: core::sync::atomic::AtomicU32::new(0) })
}

/// Put one frame in the ring and wait for the card to say it went.
///
/// Returns the number of polls it took, which is worth reporting: it is the
/// difference between "the card did it" and "we assumed it did".
///
/// # Safety
/// `ring` must come from [`tx_init`] and the register window must still be
/// mapped.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn tx_send(ring: &TxRing, frame: &[u8]) -> Result<u32, TxError> {
    if frame.len() > 2048 {
        return Err(TxError::FrameTooLong);
    }

    // SAFETY: the buffer frame is owned by the kernel and below the identity
    // map, so this writes into it through its physical address.
    unsafe {
        core::ptr::copy_nonoverlapping(
            frame.as_ptr(),
            cpu_addr(ring.buffer_phys) as *mut u8,
            frame.len(),
        );
    }

    let index = ring.next.load(core::sync::atomic::Ordering::Relaxed) as usize % TX_RING_LEN;

    // The descriptor at the cursor: address, length, command. The status byte
    // stays zero so the card setting it is unambiguous.
    // SAFETY: as above, and the ring is `TX_RING_LEN * TXD_SIZE` bytes, so
    // `index` being below `TX_RING_LEN` keeps this inside it.
    unsafe {
        // The descriptor is written by the CPU, so `cpu_addr`; the value put *into*
        // it below is the buffer's physical address, because the card reads that.
        let d = (cpu_addr(ring.ring_phys) as *mut u8).add(index * TXD_SIZE);
        core::ptr::write_volatile(d.cast::<u64>(), ring.buffer_phys);
        core::ptr::write_volatile(d.add(8).cast::<u16>(), frame.len() as u16);
        core::ptr::write_volatile(d.add(10), 0u8); // checksum offset: none
        core::ptr::write_volatile(d.add(11), TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS);
        core::ptr::write_volatile(d.add(12), 0u8); // status, cleared for us to watch
        core::ptr::write_volatile(d.add(13), 0u8);
        core::ptr::write_volatile(d.add(14).cast::<u16>(), 0u16);
    }

    // Moving the tail past this descriptor is what tells the card to look.
    // SAFETY: the window is mapped and the offset is inside it.
    unsafe { write_reg(REG_TDT, ((index + 1) % TX_RING_LEN) as u32) };
    ring.next.store(((index + 1) % TX_RING_LEN) as u32, core::sync::atomic::Ordering::Relaxed);

    // Poll for the card's acknowledgement rather than wait for its interrupt.
    // The line is unmasked by the time this runs, but a send that completes in
    // one read is not worth an interrupt, and the transmit cause is not among
    // the ones enabled. The budget is large enough that exhausting it means
    // something is wrong, not slow.
    for polls in 1..=1_000_000u32 {
        // SAFETY: the descriptor is in the kernel's own frame, at the cursor.
        let status = unsafe {
            core::ptr::read_volatile(
                (cpu_addr(ring.ring_phys) as *const u8).add(index * TXD_SIZE + 12),
            )
        };
        if status & TXD_STA_DD != 0 {
            return Ok(polls);
        }
        core::hint::spin_loop();
    }
    Err(TxError::NotSentInTime)
}

/// The sender's IP in the ARP request. QEMU's user networking hands this address
/// to the guest, so it is the natural one to claim even on a machine where that
/// networking is not compiled in.
pub const ARP_SENDER_IP: [u8; 4] = [10, 0, 2, 15];
/// The address being asked about: the gateway QEMU's user networking provides.
pub const ARP_TARGET_IP: [u8; 4] = [10, 0, 2, 2];
/// Ethernet's minimum frame size without the checksum. Built to this length
/// here rather than relying on the card to pad, so what leaves is exactly what
/// this code decided.
pub const ETH_MIN_FRAME: usize = 60;

/// Build an ARP request: "who has `ARP_TARGET_IP`, tell `mac`".
///
/// Written out byte by byte rather than through a struct, because the layout is
/// the specification and a `#[repr(C)]` struct would hide a padding mistake.
#[must_use]
pub fn build_arp_request(mac: &[u8; 6]) -> [u8; ETH_MIN_FRAME] {
    let mut f = [0u8; ETH_MIN_FRAME];
    f[0..6].copy_from_slice(&[0xFF; 6]); // broadcast
    f[6..12].copy_from_slice(mac); // source
    f[12..14].copy_from_slice(&[0x08, 0x06]); // ethertype: ARP
    f[14..16].copy_from_slice(&[0x00, 0x01]); // hardware type: Ethernet
    f[16..18].copy_from_slice(&[0x08, 0x00]); // protocol type: IPv4
    f[18] = 6; // hardware address length
    f[19] = 4; // protocol address length
    f[20..22].copy_from_slice(&[0x00, 0x01]); // opcode: request
    f[22..28].copy_from_slice(mac); // sender hardware address
    f[28..32].copy_from_slice(&ARP_SENDER_IP);
    // target hardware address stays zero: that is what is being asked
    f[38..42].copy_from_slice(&ARP_TARGET_IP);
    // 42..60 stays zero: padding to the Ethernet minimum
    f
}

// ----------------------------------------------------------------- receive

/// Receive Control. Bit 1 enables the receiver; bit 15 accepts broadcast.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_RCTL: u64 = 0x0100;
/// Receive Descriptor Base Address, low and high halves.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_RDBAL: u64 = 0x2800;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_RDBAH: u64 = 0x2804;
/// Receive Descriptor ring Length in bytes; a multiple of 128.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_RDLEN: u64 = 0x2808;
/// Receive Descriptor Head -- the card's cursor.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_RDH: u64 = 0x2810;
/// Receive Descriptor Tail -- ours. It marks the last descriptor the card may
/// write into, so it trails the ring by one.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const REG_RDT: u64 = 0x2818;

/// RCTL.EN -- receiver enabled.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RCTL_EN: u32 = 1 << 1;
/// RCTL.BAM -- accept broadcast. Without it the card drops every broadcast
/// frame, which is most of what an idle network carries and all of what an ARP
/// request is.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RCTL_BAM: u32 = 1 << 15;
/// RCTL.SECRC -- strip the Ethernet checksum, so what lands in the buffer is
/// the frame and nothing else.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RCTL_SECRC: u32 = 1 << 26;
/// Buffer size bits 17:16 at `00` mean 2048 bytes, which is the reset value and
/// what the buffers below are sized for.

/// Receive descriptor status: descriptor done, the card filled this one in.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RXD_STA_DD: u8 = 1 << 0;
/// Receive descriptor status: end of packet.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RXD_STA_EOP: u8 = 1 << 1;

/// Descriptors in the receive ring. Sixteen at 16 bytes is 256, a multiple of
/// the 128 the card requires.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RX_RING_LEN: usize = 16;
/// Each descriptor gets its own 2 KiB buffer, and two fit in a 4 KiB frame.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const RX_BUFFER_SIZE: u64 = 2048;

/// The receive ring and the buffers behind it.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub struct RxRing {
    ring_phys: u64,
    /// Physical base of the buffer area: `RX_RING_LEN` buffers, laid out one
    /// after another.
    buffers_phys: u64,
}

/// A frame the card put in memory.
#[derive(Clone, Copy)]
pub struct RxFrame {
    pub bytes: [u8; 64],
    /// How long the frame actually was; `bytes` holds the first 64 of it.
    pub len: usize,
}

/// What a wait did, whether or not anything arrived. The numbers are here so
/// that a wait which found nothing can still say what it spent, which is the
/// only way to tell a network that stayed quiet from a budget that ran out.
pub struct RxOutcome {
    pub frame: Option<RxFrame>,
    /// How many times the whole ring was scanned.
    pub polls: u64,
    /// Cycles actually spent waiting.
    pub cycles: u64,
    /// Cycles the wait was allowed.
    pub budget: u64,
    /// What the link looked like while waiting -- see [`LinkWatch`].
    pub link: LinkWatch,
}

/// When the card started saying the link was ready, relative to the start of
/// the wait.
///
/// Each field is a cycle count, or [`LinkWatch::ALREADY`] if it was true before
/// the wait began, or [`LinkWatch::NEVER`] if it never became true. The point is
/// to be able to compare those moments against the moment a received frame
/// finally appears, rather than assume they are related.
#[derive(Clone, Copy)]
pub struct LinkWatch {
    /// `STATUS` bit 1, the card's own view of the link.
    pub status_lu: u64,
    /// MII status bit 2, the PHY's view of the link.
    pub phy_link: u64,
    /// MII status bit 5, auto-negotiation finished.
    pub phy_autoneg: u64,
    /// How many times the PHY was asked and did not answer.
    pub phy_unreadable: u32,
    /// The first MII status word read, kept raw.
    ///
    /// Without it, "the PHY says the link is up" is not evidence: a PHY that is
    /// not there reads as `0xFFFF`, every bit set, which says the link is up and
    /// everything else too. A word with some bits set and some clear is a real
    /// answer from a real device.
    pub phy_first_word: u16,
    /// Whether that first word was read at all.
    pub phy_answered: bool,
}

impl LinkWatch {
    /// It was already true when the wait began.
    pub const ALREADY: u64 = 0;
    /// It never became true while the wait lasted.
    pub const NEVER: u64 = u64::MAX;
}

/// Set up the receive ring and turn the receiver on.
///
/// # Safety
/// The register window must be mapped and bus mastering must be on, for the
/// same reason [`tx_init`] needs them.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn rx_init() -> Result<RxRing, TxError> {
    let ring_phys = crate::mm::frame_alloc::alloc_frame().ok_or(TxError::NoFrame)?;
    // Sixteen 2 KiB buffers is 32 KiB, which is eight frames. They have to be
    // contiguous, so they are taken in a row and checked.
    let first = crate::mm::frame_alloc::alloc_frame().ok_or(TxError::NoFrame)?;
    let mut last = first;
    for _ in 1..8 {
        let next = crate::mm::frame_alloc::alloc_frame().ok_or(TxError::NoFrame)?;
        if next != last + 4096 {
            // The allocator handed out a frame that is not adjacent. Rather than
            // quietly using a ring whose buffers are not where the card will be
            // told they are, stop.
            return Err(TxError::FrameAboveIdentityMap);
        }
        last = next;
    }
    if !cpu_can_reach(ring_phys) || !cpu_can_reach(last + 4096) {
        return Err(TxError::FrameAboveIdentityMap);
    }

    // SAFETY: every frame came from the allocator, and `cpu_addr` gives the
    // address this boot path can write through. The ring is zeroed so no
    // descriptor starts with a stale done bit.
    unsafe {
        core::ptr::write_bytes(cpu_addr(ring_phys) as *mut u8, 0, RX_RING_LEN * TXD_SIZE);
        for i in 0..RX_RING_LEN {
            let d = (cpu_addr(ring_phys) as *mut u8).add(i * TXD_SIZE);
            core::ptr::write_volatile(
                d.cast::<u64>(),
                first + (i as u64) * RX_BUFFER_SIZE,
            );
        }
    }

    // SAFETY: the window is mapped by the caller's contract, and every offset
    // is inside it -- see the assertion next to `MMIO_PAGES`.
    unsafe {
        write_reg(REG_RDBAL, (ring_phys & 0xFFFF_FFFF) as u32);
        write_reg(REG_RDBAH, (ring_phys >> 32) as u32);
        write_reg(REG_RDLEN, (RX_RING_LEN * TXD_SIZE) as u32);
        write_reg(REG_RDH, 0);
        // The tail is the last descriptor the card may use, so it trails the
        // head by one: with it equal to the head the ring reads as empty and
        // nothing is ever received.
        write_reg(REG_RDT, (RX_RING_LEN - 1) as u32);
        write_reg(REG_RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
    }

    Ok(RxRing { ring_phys, buffers_phys: first })
}

/// Wait for one frame, for at most `millis` of wall time.
///
/// Bounded by time rather than by a poll count on purpose. The first version
/// counted polls, and the number that felt generous -- four million -- turned a
/// five-second boot into seventy when nothing answered, because a poll costs
/// whatever the machine says it costs and not what I assumed. A deadline says
/// what it means.
///
/// # Safety
/// `ring` must come from [`rx_init`] and the window must still be mapped.
/// Read one PHY register through MDIC.
///
/// The PHY is a second device behind the card, reached one register at a time:
/// write the address and the operation, wait for the card to set the ready bit,
/// then take the answer from the low half. Returns `None` if the card reports
/// an error or does not answer, rather than handing back a zero that would read
/// as a perfectly good "nothing is up".
///
/// # Safety
///
/// The register window must be mapped.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn phy_read(reg: u32) -> Option<u16> {
    // SAFETY: the caller's contract.
    unsafe {
        write_reg(
            REG_MDIC,
            (reg << 16) | (PHY_ADDRESS << 21) | MDIC_OP_READ,
        );
    }
    // Bounded: a PHY that never answers must not stop the boot. The manual asks
    // for tens of microseconds; this is far more than that and still finite.
    for _ in 0..100_000u32 {
        // SAFETY: as above.
        let mdic = unsafe { read_reg(REG_MDIC) };
        if mdic & MDIC_ERROR != 0 {
            return None;
        }
        if mdic & MDIC_READY != 0 {
            return Some((mdic & 0xFFFF) as u16);
        }
        core::hint::spin_loop();
    }
    None
}

/// A card that has been set up and is waiting to be asked something.
///
/// The boot enables the receiver as early as it can and sends its question as
/// late as it can, because enabling the receiver starts a timer in the emulated
/// card during which nothing is delivered -- see the note in [`rx_poll`]. Those
/// two moments are not even on the same stack: the question is sent from the
/// end of the boot, which in the demo build is reached from inside a syscall
/// handler after the kernel stack that set the card up has been abandoned. So
/// this has to outlive both.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub struct Armed {
    pub tx: TxRing,
    pub rx: Option<RxRing>,
    pub mac: [u8; 6],
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
struct ArmedCell(core::cell::UnsafeCell<Option<Armed>>);

// SAFETY: written exactly once, from the boot path, on the bootstrap CPU,
// before any other CPU is started and before anything can read it; read-only
// afterwards. `store_armed` refuses a second write rather than trusting that.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
unsafe impl Sync for ArmedCell {}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static ARMED: ArmedCell = ArmedCell(core::cell::UnsafeCell::new(None));

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static ARMED_SET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Hand the armed card over for the rest of the boot to use.
///
/// Only the first call takes; a second is ignored rather than overwriting rings
/// the card may be reading from.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn store_armed(armed: Armed) {
    use core::sync::atomic::Ordering;
    if ARMED_SET
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // SAFETY: this is the only writer, and the exchange above guarantees it
    // runs once. No reader can observe the cell before `ARMED_SET` is true,
    // which is published by the release ordering above.
    unsafe {
        *ARMED.0.get() = Some(armed);
    }
}

/// The armed card, if the boot found one.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[must_use]
pub fn armed() -> Option<&'static Armed> {
    use core::sync::atomic::Ordering;
    if !ARMED_SET.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: `ARMED_SET` is only true after the single write completed, and
    // nothing writes afterwards, so this is a shared read of a value that is
    // now immutable.
    unsafe { (*ARMED.0.get()).as_ref() }
}

/// Set once the register window is mapped and the card has been told to raise
/// its line. Until then the handler must not touch a register: the interrupt it
/// sees belongs to something else sharing IRQ 11.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static IRQ_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// How many times the card has reported a received frame.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static RX_IRQS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The last cause word read, kept because reading clears it and the boot report
/// is written long after the handler ran.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static LAST_ICR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Let the card raise IRQ 11 when a frame arrives.
///
/// # Safety
///
/// The register window must be mapped, as for every other call here. The
/// caller must also have installed a handler and unmasked the line first: a
/// card allowed to assert an interrupt nobody acknowledges will hold the line
/// down and the machine will stop taking any interrupt at all.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn enable_rx_interrupt() {
    // SAFETY: the caller's contract.
    unsafe {
        // Clear anything pending first, so the first interrupt reported is one
        // that happened after this point.
        let _ = read_reg(REG_ICR);
        write_reg(REG_IMS, ICR_RXT0 | ICR_RXDMT0);
    }
    IRQ_ARMED.store(true, core::sync::atomic::Ordering::Release);
}

/// Acknowledge an interrupt on the shared line.
///
/// Returns true when it was this card's. IRQ 11 is shared on a PC, so a handler
/// that assumed every interrupt was the card's would swallow other devices'.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn on_interrupt() -> bool {
    use core::sync::atomic::Ordering;
    if !IRQ_ARMED.load(Ordering::Acquire) {
        return false;
    }
    // SAFETY: `IRQ_ARMED` is only set after the window is mapped, and it is
    // never cleared, so the mapping is live for as long as this can run.
    let icr = unsafe { read_reg(REG_ICR) };
    if icr == 0 {
        return false;
    }
    LAST_ICR.store(icr, Ordering::Relaxed);
    if icr & (ICR_RXT0 | ICR_RXDMT0) != 0 {
        RX_IRQS.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// How many receive interrupts have been seen.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn rx_irq_count() -> u32 {
    RX_IRQS.load(core::sync::atomic::Ordering::Relaxed)
}

/// The last cause word, for the boot report.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn last_icr() -> u32 {
    LAST_ICR.load(core::sync::atomic::Ordering::Relaxed)
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub unsafe fn rx_poll(ring: &RxRing, millis: u64) -> RxOutcome {
    // A floor for the clock rate, as elsewhere in this kernel: on a faster part
    // the wait is shorter, which is the harmless direction.
    const MIN_TSC_HZ: u64 = 1_500_000_000;
    let budget = MIN_TSC_HZ / 1000 * millis;
    // SAFETY: `_rdtsc` is unsafe because it is a target intrinsic, not because
    // it touches anything; this crate is built for x86_64 and the counter has
    // been readable on every part since the Pentium. It is read the same way
    // inside the loop, under the same reasoning.
    let started = unsafe { core::arch::x86_64::_rdtsc() };
    // Why this wait is long on QEMU, and what it is anchored to. All measured,
    // and the first three findings are things ruled OUT:
    //
    //   * on the wire the reply is there 0.06-0.1 ms after the request, in every
    //     capture taken -- so it is not the network;
    //   * waiting with `hlt` instead of spinning changes the number of scans
    //     from about three million to a hundred and the elapsed time not at
    //     all -- so it is not this loop denying the emulator its turn;
    //   * the card and its PHY both report the link up from the first read, and
    //     auto-negotiation never reports finished at all, so nothing about the
    //     link changes at the moment the frame appears.
    //
    // What it IS anchored to is the write that enables the receiver, `RCTL` in
    // `rx_init`. Delaying everything from before that write by 1.5e9 cycles, and
    // then by 3.0e9, left this wait unchanged at about 3.19e9 both times while
    // the boot got correspondingly longer. An earlier test that delayed only the
    // question could not tell this apart from a fixed point after power-on,
    // because it left both on the near side of the delay.
    //
    // From the difference between those two runs -- half a second of wall clock
    // per 1.5e9 cycles -- the counter here runs at about 3 GHz, which puts this
    // wait at roughly 1.05 seconds. A one-second timer, started by enabling the
    // receiver, during which the device model accepts nothing.
    //
    // The consequence for a driver: arm the receiver early and ask late. This
    // kernel does the opposite, enabling and asking within microseconds of each
    // other, so it pays the whole second. The console build shows the other side
    // of it: a request typed after boot is answered on the first scan.
    let mut polls: u64 = 0;

    // Watch the link while waiting. The question this answers is whether the
    // moment a frame becomes visible is the moment the card or its PHY says it
    // is ready -- which would make the delay auto-negotiation -- or some other
    // moment entirely. Sampling costs one register read and one PHY exchange
    // per turn of a loop that turns about a hundred times.
    let mut link = LinkWatch {
        status_lu: LinkWatch::NEVER,
        phy_link: LinkWatch::NEVER,
        phy_autoneg: LinkWatch::NEVER,
        phy_unreadable: 0,
        phy_first_word: 0,
        phy_answered: false,
    };
    {
        // SAFETY: the window is mapped by the caller's contract.
        let status = unsafe { read_reg(REG_STATUS) };
        if status_link_up(status) {
            link.status_lu = LinkWatch::ALREADY;
        }
        // SAFETY: as above.
        if let Some(mii) = unsafe { phy_read(PHY_REG_STATUS) } {
            link.phy_first_word = mii;
            link.phy_answered = true;
            if mii & MII_LINK_UP != 0 {
                link.phy_link = LinkWatch::ALREADY;
            }
            if mii & MII_AUTONEG_DONE != 0 {
                link.phy_autoneg = LinkWatch::ALREADY;
            }
        } else {
            link.phy_unreadable += 1;
        }
    }
    loop {
        // SAFETY: as above.
        let spent = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(started);
        if spent >= budget {
            return RxOutcome { frame: None, polls, cycles: spent, budget, link };
        }
        polls += 1;

        if link.status_lu == LinkWatch::NEVER {
            // SAFETY: the window is mapped for as long as `ring` is alive.
            if status_link_up(unsafe { read_reg(REG_STATUS) }) {
                link.status_lu = spent;
            }
        }
        if link.phy_link == LinkWatch::NEVER || link.phy_autoneg == LinkWatch::NEVER {
            // SAFETY: as above.
            match unsafe { phy_read(PHY_REG_STATUS) } {
                Some(mii) => {
                    if link.phy_link == LinkWatch::NEVER && mii & MII_LINK_UP != 0 {
                        link.phy_link = spent;
                    }
                    if link.phy_autoneg == LinkWatch::NEVER && mii & MII_AUTONEG_DONE != 0 {
                        link.phy_autoneg = spent;
                    }
                }
                None => link.phy_unreadable += 1,
            }
        }
        for i in 0..RX_RING_LEN {
            // SAFETY: the descriptor is in the kernel's own frame, below the
            // identity map.
            let (status, len) = unsafe {
                let d = (cpu_addr(ring.ring_phys) as *const u8).add(i * TXD_SIZE);
                (
                    core::ptr::read_volatile(d.add(12)),
                    core::ptr::read_volatile(d.add(8).cast::<u16>()),
                )
            };
            if status & RXD_STA_DD == 0 || status & RXD_STA_EOP == 0 {
                continue;
            }
            let mut frame = RxFrame { bytes: [0u8; 64], len: len as usize };
            let copy = core::cmp::min(frame.len, frame.bytes.len());
            // SAFETY: the buffer for descriptor `i` is `RX_BUFFER_SIZE` bytes at
            // a known offset, and `copy` is at most 64.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    cpu_addr(ring.buffers_phys + (i as u64) * RX_BUFFER_SIZE) as *const u8,
                    frame.bytes.as_mut_ptr(),
                    copy,
                );
                // Hand the descriptor back: clear the status and move the tail
                // past it, or the card stops after one ring's worth.
                let d = (cpu_addr(ring.ring_phys) as *mut u8).add(i * TXD_SIZE);
                core::ptr::write_volatile(d.add(12), 0u8);
                write_reg(REG_RDT, i as u32);
            }
            // SAFETY: as above.
            let spent = unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(started);
            return RxOutcome { frame: Some(frame), polls, cycles: spent, budget, link };
        }
        // Hand the machine back rather than spinning. This does not make the
        // reply arrive sooner -- measured, see the note above the loop -- but it
        // does the same waiting in 102 scans instead of three million, so the
        // kernel is not burning a core for two seconds to no purpose.
        // SAFETY: `hlt` only when IF is set AND something is going to arrive.
        //
        // IF says interrupts are allowed, not that any source exists, and the two
        // are not the same kernel. On the Limine boot path this one enables
        // interrupts and never starts the PIT -- zero `[ZPL-SCHED]` lines in the
        // log prove it -- so a halt there is permanent. Measured: an ARP reply that
        // was on the wire and never collected, with the boot stopped in this loop.
        unsafe {
            let flags: u64;
            core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, nostack));
            let can_wake = flags & (1 << 9) != 0 && crate::interrupts::timer_has_fired();
            if can_wake {
                core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
            } else {
                core::hint::spin_loop();
            }
        }
    }
}

/// True when the frame is an ARP reply that answers [`ARP_SENDER_IP`].
///
/// Checked here rather than at the call site so the condition sits next to the
/// frame builder it mirrors: opcode 2 where the request had 1, and the sender
/// of the reply is the address the request asked about.
#[must_use]
pub fn is_arp_reply_for_us(frame: &[u8]) -> bool {
    frame.len() >= 42
        && frame[12] == 0x08
        && frame[13] == 0x06
        && frame[20] == 0x00
        && frame[21] == 0x02
        && frame[28..32] == ARP_TARGET_IP
        && frame[38..42] == ARP_SENDER_IP
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(vendor: u16, device_id: u16, bar0_raw: u32) -> PciDevice {
        PciDevice {
            bus: 0, device: 3, function: 0, vendor, device_id,
            class: 0x02, subclass: 0x00, prog_if: 0x00, header_type: 0x00,
            bar0_raw, interrupt_line: 11,
        }
    }

    #[test]
    fn the_card_qemu_puts_in_its_default_machine_is_recognised() {
        assert!(is_e1000(&dev(0x8086, 0x100E, 0xFEBC_0000)));
    }

    #[test]
    fn another_intel_device_is_not_this_card() {
        // The default machine has several 8086 devices; matching on vendor
        // alone would map the wrong BAR into the page tables.
        assert!(!is_e1000(&dev(0x8086, 0x7010, 0)));
    }

    #[test]
    fn a_memory_bar_gives_its_base_with_the_flag_bits_cleared() {
        // The real value QEMU reported for this card.
        assert_eq!(memory_bar_base(0xFEBC_0000), Some(0xFEBC_0000));
        // Low four bits are flags, not address.
        assert_eq!(memory_bar_base(0xFD00_0008), Some(0xFD00_0000));
    }

    #[test]
    fn an_io_bar_is_refused_rather_than_masked_into_an_address() {
        // Bit 0 set means I/O space. Masking it off would produce a plausible
        // physical address that is not one, and map something arbitrary.
        assert_eq!(memory_bar_base(0x0000_C001), None);
    }

    #[test]
    fn an_unset_bar_is_refused() {
        assert_eq!(memory_bar_base(0), None);
        assert_eq!(memory_bar_base(0xFFFF_FFFF), None);
    }

    #[test]
    fn the_qemu_prefix_is_recognised_and_nothing_else_is() {
        let qemu = MacAddress { bytes: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56], valid: true };
        assert!(qemu.looks_like_qemu());
        let intel = MacAddress { bytes: [0x00, 0x1B, 0x21, 0x12, 0x34, 0x56], valid: true };
        assert!(!intel.looks_like_qemu());
    }

    #[test]
    fn link_up_is_bit_one_of_the_status_register() {
        assert!(status_link_up(0b10));
        assert!(!status_link_up(0b01));
    }
}

#[cfg(test)]
mod arp_tests {
    use super::*;

    const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    #[test]
    fn the_frame_is_the_ethernet_minimum() {
        // Built to 60 here rather than relying on the card's padding, so what
        // leaves the machine is exactly what this code decided.
        assert_eq!(build_arp_request(&MAC).len(), 60);
    }

    #[test]
    fn it_is_broadcast_from_our_own_address() {
        let f = build_arp_request(&MAC);
        assert_eq!(&f[0..6], &[0xFF; 6]);
        assert_eq!(&f[6..12], &MAC);
    }

    #[test]
    fn the_ethertype_is_arp() {
        assert_eq!(&build_arp_request(&MAC)[12..14], &[0x08, 0x06]);
    }

    #[test]
    fn it_is_a_request_for_ipv4_over_ethernet() {
        let f = build_arp_request(&MAC);
        assert_eq!(&f[14..16], &[0x00, 0x01], "hardware type: Ethernet");
        assert_eq!(&f[16..18], &[0x08, 0x00], "protocol type: IPv4");
        assert_eq!(f[18], 6, "hardware address length");
        assert_eq!(f[19], 4, "protocol address length");
        assert_eq!(&f[20..22], &[0x00, 0x01], "opcode: request, not reply");
    }

    #[test]
    fn the_addresses_are_the_ones_the_proof_looks_for() {
        let f = build_arp_request(&MAC);
        assert_eq!(&f[22..28], &MAC, "sender hardware address");
        assert_eq!(&f[28..32], &ARP_SENDER_IP);
        assert_eq!(&f[32..38], &[0u8; 6], "target hardware address is the question");
        assert_eq!(&f[38..42], &ARP_TARGET_IP);
    }

    #[test]
    fn the_padding_is_zero_and_carries_nothing() {
        // A frame padded with whatever was in the buffer would leak kernel
        // memory onto the wire. Eighteen bytes of it, every time.
        assert_eq!(&build_arp_request(&MAC)[42..60], &[0u8; 18]);
    }
}

#[cfg(test)]
mod rx_tests {
    use super::*;

    const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

    /// An ARP reply from 10.0.2.2, the shape the peer script sends.
    fn reply() -> [u8; 42] {
        let mut f = [0u8; 42];
        f[0..6].copy_from_slice(&MAC);
        f[6..12].copy_from_slice(&[0x52, 0x55, 0x0A, 0x00, 0x02, 0x02]);
        f[12..14].copy_from_slice(&[0x08, 0x06]);
        f[14..16].copy_from_slice(&[0x00, 0x01]);
        f[16..18].copy_from_slice(&[0x08, 0x00]);
        f[18] = 6;
        f[19] = 4;
        f[20..22].copy_from_slice(&[0x00, 0x02]); // reply
        f[22..28].copy_from_slice(&[0x52, 0x55, 0x0A, 0x00, 0x02, 0x02]);
        f[28..32].copy_from_slice(&ARP_TARGET_IP); // from the address we asked about
        f[32..38].copy_from_slice(&MAC);
        f[38..42].copy_from_slice(&ARP_SENDER_IP); // to us
        f
    }

    #[test]
    fn a_reply_to_our_question_is_recognised() {
        assert!(is_arp_reply_for_us(&reply()));
    }

    #[test]
    fn our_own_request_is_not_mistaken_for_a_reply() {
        // The card accepts broadcast, so the request may come back around. An
        // opcode check is what keeps the kernel from answering itself.
        assert!(!is_arp_reply_for_us(&build_arp_request(&MAC)));
    }

    #[test]
    fn a_reply_about_some_other_address_is_not_ours() {
        let mut f = reply();
        f[28..32].copy_from_slice(&[10, 0, 2, 99]);
        assert!(!is_arp_reply_for_us(&f));
    }

    #[test]
    fn a_reply_addressed_to_someone_else_is_not_ours() {
        let mut f = reply();
        f[38..42].copy_from_slice(&[10, 0, 2, 77]);
        assert!(!is_arp_reply_for_us(&f));
    }

    #[test]
    fn a_frame_too_short_to_hold_an_arp_header_is_refused() {
        // Without the length check this would read past the end of the slice.
        assert!(!is_arp_reply_for_us(&reply()[..20]));
    }
}
