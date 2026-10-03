//! A minimal xHCI host controller driver: enough to find one USB boot keyboard on a
//! root port and read its reports by polling.
//!
//! What it does, in the order the xHCI 1.2 specification (section 4.2) asks for it:
//!
//! 1. take the controller from the firmware, if the firmware holds it;
//! 2. stop and reset it;
//! 3. give it a device context table, a command ring and one event ring;
//! 4. run it, and reset each root port that has something connected;
//! 5. for each such device: enable a slot, address it, read its descriptors, and stop
//!    at the first one that is a boot keyboard;
//! 6. configure that keyboard's interrupt IN endpoint, switch it to the boot protocol,
//!    and keep one 8-byte read queued on it.
//!
//! What it does not do: interrupts (the event ring is read by polling, from the halt
//! loop, and the interrupter is never enabled), hubs (a keyboard behind a hub is not
//! found), more than one device, unplugging, or any other kind of device.
//!
//! # Safety
//!
//! All memory and register access goes through `super::dma` and `super::mmio`. The
//! only `unsafe` here is in [`bring_up`]: reading the BAR, mapping the registers it
//! names, and switching on the device's DMA -- the three calls that hand this module a
//! device, each justified where it is made.

use super::dma::DmaPage;
use super::hid::{self, BootReportTranslator, KeyboardInterface, ScancodeQueue};
use super::mmio::{RegisterWindow, MAX_LEN};
use super::trb::{self, SetupPacket, Trb};
use crate::drivers::pci::{self, PciDevice};

// ---------------------------------------------------------------------------
// Registers (xHCI 1.2 chapter 5)
// ---------------------------------------------------------------------------

// Capability registers, from the start of the window.
const CAP_LENGTH: u64 = 0x00; // byte 0; HCIVERSION in the upper half
const HCSPARAMS1: u64 = 0x04;
const HCSPARAMS2: u64 = 0x08;
const HCCPARAMS1: u64 = 0x10;
const DBOFF: u64 = 0x14;
const RTSOFF: u64 = 0x18;

// Operational registers, from `op`.
const USBCMD: u64 = 0x00;
const USBSTS: u64 = 0x04;
const PAGESIZE: u64 = 0x08;
const CRCR: u64 = 0x18;
const DCBAAP: u64 = 0x30;
const CONFIG: u64 = 0x38;
const PORTSC_BASE: u64 = 0x400;
const PORT_STRIDE: u64 = 0x10;

const CMD_RUN: u32 = 1 << 0;
const CMD_RESET: u32 = 1 << 1;
const STS_HALTED: u32 = 1 << 0;
const STS_NOT_READY: u32 = 1 << 11;

// Interrupter 0, from `rt`.
const IR0: u64 = 0x20;
const ERSTSZ: u64 = IR0 + 0x08;
const ERSTBA: u64 = IR0 + 0x10;
const ERDP: u64 = IR0 + 0x18;
/// Event Handler Busy: written as 1 with ERDP to say the handler has caught up.
const ERDP_EHB: u64 = 1 << 3;

// PORTSC bits.
const PORT_CONNECTED: u32 = 1 << 0;
const PORT_ENABLED: u32 = 1 << 1;
const PORT_RESET: u32 = 1 << 4;
const PORT_POWER: u32 = 1 << 9;
const PORT_RESET_CHANGE: u32 = 1 << 21;
/// All the write-1-to-clear change bits (CSC, PEC, WRC, OCC, PRC, PLC, CEC).
const PORT_CHANGE_BITS: u32 = 0x00FE_0000;
/// The bits a write must carry back unchanged so it changes nothing by accident:
/// the read-only ones (connected, over-current, speed, cold-attach, removable; ignored
/// on write) and the read-write ones (link state, power, indicator, the three wake
/// enables). Everything else is either write-1-to-clear, where writing back a 1 clears
/// it, or PED, where writing a 1 *disables* the port. The same set Linux calls the
/// "neutral" port state.
const PORT_PRESERVE: u32 = PORT_READ_ONLY | PORT_READ_WRITE;
const PORT_READ_ONLY: u32 = (1 << 0) | (1 << 3) | (0xF << 10) | (1 << 24) | (1 << 30);
const PORT_READ_WRITE: u32 = (0xF << 5) | PORT_POWER | (0x3 << 14) | (0x7 << 25);

// Extended capability "USB Legacy Support" (id 1).
const XCAP_LEGACY: u32 = 1;
const LEGACY_BIOS_OWNED: u32 = 1 << 16;
const LEGACY_OS_OWNED: u32 = 1 << 24;
/// In USBLEGCTLSTS, the bits that are not SMI enables: kept when switching SMIs off.
const LEGACY_SMI_KEEP: u32 = (0x7 << 1) | (0xFF << 5) | (0x7 << 17);
/// And the SMI event bits, written as 1 to clear them.
const LEGACY_SMI_EVENTS: u32 = 0x7 << 29;

/// TRBs per ring: one 4 KiB page of 16-byte TRBs.
const RING_TRBS: usize = 256;

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Timestamp-counter ticks taken to be one millisecond.
///
/// There is no calibrated clock to ask, so this assumes the fastest counter likely,
/// 4 GHz. On a slower one every wait lasts *longer* than asked, never shorter, which is
/// the safe way round for both timeouts and the settling times USB requires.
const TSC_PER_MS: u64 = 4_000_000;

fn now() -> u64 {
    crate::audit::timing::rdtsc()
}

fn sleep_ms(ms: u64) {
    let start = now();
    while now().wrapping_sub(start) < ms * TSC_PER_MS {
        core::hint::spin_loop();
    }
}

/// Spin until `done` says so or `ms` milliseconds pass. True if it said so.
fn wait_for(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let start = now();
    loop {
        if done() {
            return true;
        }
        if now().wrapping_sub(start) >= ms * TSC_PER_MS {
            return false;
        }
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Rings
// ---------------------------------------------------------------------------

/// A producer ring: the command ring, or a transfer ring. One page, with a Link TRB in
/// the last slot that sends the controller back to the start.
struct Ring {
    page: DmaPage,
    index: usize,
    cycle: bool,
}

impl Ring {
    fn new() -> Option<Self> {
        let page = DmaPage::alloc()?;
        let link = trb::link(page.phys());
        let at = (RING_TRBS - 1) * 16;
        page.write64(at, link.param);
        page.write32(at + 8, link.status);
        // Cycle bit 0: the controller, which starts expecting 1, does not follow it
        // until `push` hands it over on the first wrap.
        page.write32(at + 12, link.control);
        Some(Self { page, index: 0, cycle: true })
    }

    fn phys(&self) -> u64 {
        self.page.phys()
    }

    /// Write one TRB and return its physical address. The control word, which carries
    /// the cycle bit that hands the TRB to the controller, is written last.
    fn push(&mut self, t: Trb) -> u64 {
        let at = self.index * 16;
        let addr = self.page.phys() + at as u64;
        self.page.write64(at, t.param);
        self.page.write32(at + 8, t.status);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.page.write32(at + 12, t.control | u32::from(self.cycle));
        self.index += 1;
        if self.index == RING_TRBS - 1 {
            // Hand over the link with the current cycle, then flip ours.
            let link = (RING_TRBS - 1) * 16 + 12;
            let control = self.page.read32(link) & !trb::CYCLE;
            self.page.write32(link, control | u32::from(self.cycle));
            self.cycle = !self.cycle;
            self.index = 0;
        }
        addr
    }
}

/// The event ring: the controller produces, this consumes. One segment, one page.
struct EventRing {
    page: DmaPage,
    index: usize,
    cycle: bool,
}

impl EventRing {
    fn new() -> Option<Self> {
        Some(Self { page: DmaPage::alloc()?, index: 0, cycle: true })
    }

    /// The next event, if the controller has written one.
    fn pop(&mut self) -> Option<Trb> {
        let at = self.index * 16;
        let control = self.page.read32(at + 12);
        if (control & trb::CYCLE != 0) != self.cycle {
            return None;
        }
        let t = Trb { param: self.page.read64(at), status: self.page.read32(at + 8), control };
        self.index += 1;
        if self.index == RING_TRBS {
            self.index = 0;
            self.cycle = !self.cycle;
        }
        Some(t)
    }

    fn dequeue_phys(&self) -> u64 {
        self.page.phys() + (self.index * 16) as u64
    }
}

// ---------------------------------------------------------------------------
// The controller
// ---------------------------------------------------------------------------

/// Why bringing the keyboard up stopped. Each names the step, which is what gets
/// printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The machine has a PS/2 controller and the firmware is using this xHCI --
    /// most likely to emulate a PS/2 keyboard from a USB one. Taking the controller
    /// away would end that emulation, so it is left alone.
    LeftToFirmware,
    /// No connected root port held a boot keyboard.
    NoKeyboard,
    /// A step failed; the text says which.
    Step(&'static str),
}

/// What [`bring_up`] found about the controller, for the report line.
#[derive(Clone, Copy, Default)]
pub struct ControllerInfo {
    pub ports: u8,
    pub slots: u8,
    pub scratchpads: u16,
    pub context_size: u8,
}

/// The keyboard, once found: everything [`Keyboard::poll`] needs.
pub struct Keyboard {
    hc: Controller,
    slot: u8,
    dci: u8,
    ring: Ring,
    report: DmaPage,
    max_packet: u16,
    translator: BootReportTranslator,
    queue: ScancodeQueue,
    errors: u32,
    pub port: u8,
    pub speed: u8,
    pub endpoint: u8,
}

struct Controller {
    regs: RegisterWindow,
    op: u64,
    rt: u64,
    db: u64,
    info: ControllerInfo,
    dcbaa: DmaPage,
    commands: Ring,
    events: EventRing,
}

/// Find the controller's "USB Legacy Support" capability, if it has one.
fn legacy_capability(regs: &RegisterWindow) -> Option<u64> {
    let mut at = u64::from(regs.read32(HCCPARAMS1) >> 16) * 4;
    // Bounded: a malformed list that points back at itself must end.
    for _ in 0..64 {
        if at == 0 || at + 4 > regs.len() {
            return None;
        }
        let cap = regs.read32(at);
        if cap & 0xFF == XCAP_LEGACY {
            return Some(at);
        }
        let next = u64::from((cap >> 8) & 0xFF) * 4;
        if next == 0 {
            return None;
        }
        at += next;
    }
    None
}

/// The number of bytes of the register window the driver will touch.
fn window_needed(regs: &RegisterWindow) -> u64 {
    let cap_len = u64::from(regs.read32(CAP_LENGTH) & 0xFF);
    let ports = u64::from(regs.read32(HCSPARAMS1) >> 24);
    let slots = u64::from(regs.read32(HCSPARAMS1) & 0xFF);
    let db = u64::from(regs.read32(DBOFF) & !0x3);
    let rt = u64::from(regs.read32(RTSOFF) & !0x1F);
    let xecp = u64::from(regs.read32(HCCPARAMS1) >> 16) * 4;
    let ends = [
        cap_len + PORTSC_BASE + ports * PORT_STRIDE,
        rt + IR0 + 0x20,
        db + (slots + 1) * 4,
        // The extended capabilities have no stated length; one page past their start
        // covers the legacy-support one, which is the only one read.
        if xecp == 0 { 0 } else { xecp + 0x1000 },
    ];
    let mut need = 0;
    for end in ends {
        need = need.max(end);
    }
    need.min(MAX_LEN)
}

/// Bring up the controller `dev` and look for a keyboard on it.
///
/// `ps2_present` says whether the machine has an i8042. When it does and the firmware
/// holds this controller, the controller is left alone: see [`Failure::LeftToFirmware`].
pub fn bring_up(dev: &PciDevice, ps2_present: bool) -> Result<(Keyboard, ControllerInfo), Failure> {
    // SAFETY: `dev` came from the PCI walk and is an xHCI controller; reading its BARs
    // is a configuration-space read.
    let base = unsafe { pci::read_memory_bar0(dev) }.ok_or(Failure::Step("no memory BAR"))?;

    // SAFETY: `base` is this controller's BAR0, so the first page is its capability
    // registers; the window is used by nothing else (see `mmio`).
    let first = unsafe { RegisterWindow::map(base, 4096) }.map_err(|_| Failure::Step("map"))?;
    let need = window_needed(&first);
    // SAFETY: as above. Every offset the driver uses lies below `need`, which is
    // computed from the controller's own capability registers, so it is inside its BAR.
    let regs = unsafe { RegisterWindow::map(base, need.max(4096)) }
        .map_err(|_| Failure::Step("map"))?;

    let legacy = legacy_capability(&regs);
    if let Some(at) = legacy {
        if ps2_present && regs.read32(at) & LEGACY_BIOS_OWNED != 0 {
            return Err(Failure::LeftToFirmware);
        }
    }

    // SAFETY: this module is now the controller's driver; it points the controller's
    // DMA only at pages it allocated for that purpose.
    unsafe { pci::enable_memory_and_bus_master(dev) };

    if let Some(at) = legacy {
        take_from_firmware(&regs, at);
    }

    let hc = Controller::start(regs)?;
    let info = hc.info;
    let kbd = hc.find_keyboard()?;
    Ok((kbd, info))
}

/// The BIOS handoff, xHCI 1.2 section 4.22.1: ask for the controller, wait for the
/// firmware to let go, and switch off the SMIs it may still have enabled.
fn take_from_firmware(regs: &RegisterWindow, at: u64) {
    let cap = regs.read32(at);
    regs.write32(at, cap | LEGACY_OS_OWNED);
    let released = wait_for(1000, || regs.read32(at) & LEGACY_BIOS_OWNED == 0);
    if !released {
        // The firmware did not answer. Linux takes the controller anyway, and so does
        // this: the alternative is no keyboard at all.
        regs.write32(at, (regs.read32(at) & !LEGACY_BIOS_OWNED) | LEGACY_OS_OWNED);
    }
    let ctl = regs.read32(at + 4);
    regs.write32(at + 4, (ctl & LEGACY_SMI_KEEP) | LEGACY_SMI_EVENTS);
}

impl Controller {
    /// Stop, reset, set up the rings and run.
    fn start(regs: RegisterWindow) -> Result<Self, Failure> {
        let cap_len = u64::from(regs.read32(CAP_LENGTH) & 0xFF);
        let op = cap_len;
        let rt = u64::from(regs.read32(RTSOFF) & !0x1F);
        let db = u64::from(regs.read32(DBOFF) & !0x3);
        let params1 = regs.read32(HCSPARAMS1);
        let params2 = regs.read32(HCSPARAMS2);
        let cparams = regs.read32(HCCPARAMS1);
        let info = ControllerInfo {
            ports: (params1 >> 24) as u8,
            slots: (params1 & 0xFF) as u8,
            scratchpads: (((params2 >> 21) & 0x1F) << 5 | ((params2 >> 27) & 0x1F)) as u16,
            context_size: if cparams & (1 << 2) != 0 { 64 } else { 32 },
        };
        if regs.read32(op + PAGESIZE) & 1 == 0 {
            return Err(Failure::Step("controller does not take 4 KiB pages"));
        }

        // Stop, then reset.
        let cmd = regs.read32(op + USBCMD);
        regs.write32(op + USBCMD, cmd & !CMD_RUN);
        if !wait_for(100, || regs.read32(op + USBSTS) & STS_HALTED != 0) {
            return Err(Failure::Step("controller did not halt"));
        }
        regs.write32(op + USBCMD, CMD_RESET);
        // Some Intel controllers hang if their registers are read within 1 ms of the
        // reset being written.
        sleep_ms(1);
        if !wait_for(1000, || {
            regs.read32(op + USBCMD) & CMD_RESET == 0
                && regs.read32(op + USBSTS) & STS_NOT_READY == 0
        }) {
            return Err(Failure::Step("controller reset did not finish"));
        }

        let dcbaa = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
        let commands = Ring::new().ok_or(Failure::Step("out of memory"))?;
        let events = EventRing::new().ok_or(Failure::Step("out of memory"))?;
        let erst = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;

        // Scratchpad buffers: pages the controller keeps for itself. QEMU asks for
        // none; real controllers often ask for a few. Their array goes in entry 0.
        if info.scratchpads > 0 {
            if info.scratchpads > 256 {
                return Err(Failure::Step("too many scratchpad buffers"));
            }
            let array = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
            for i in 0..usize::from(info.scratchpads) {
                let page = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
                array.write64(i * 8, page.phys());
            }
            dcbaa.write64(0, array.phys());
        }

        regs.write32(op + CONFIG, u32::from(info.slots));
        regs.write64(op + DCBAAP, dcbaa.phys());
        regs.write64(op + CRCR, commands.phys() | 1);

        // One event ring segment, described by a one-entry table.
        erst.write64(0, events.page.phys());
        erst.write32(8, RING_TRBS as u32);
        regs.write32(rt + ERSTSZ, 1);
        regs.write64(rt + ERDP, events.page.phys());
        regs.write64(rt + ERSTBA, erst.phys());

        // Run. The interrupter is left disabled (IMAN.IE and USBCMD.INTE both clear):
        // events are read by polling.
        regs.write32(op + USBCMD, CMD_RUN);
        if !wait_for(100, || regs.read32(op + USBSTS) & STS_HALTED == 0) {
            return Err(Failure::Step("controller did not start"));
        }

        Ok(Self { regs, op, rt, db, info, dcbaa, commands, events })
    }

    fn portsc(&self, port: u8) -> u64 {
        self.op + PORTSC_BASE + u64::from(port - 1) * PORT_STRIDE
    }

    fn doorbell(&self, slot: u8, target: u8) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.regs.write32(self.db + u64::from(slot) * 4, u32::from(target));
    }

    /// The next event, with the dequeue pointer moved past it.
    fn next_event(&mut self) -> Option<Trb> {
        let e = self.events.pop()?;
        self.regs.write64(self.rt + ERDP, self.events.dequeue_phys() | ERDP_EHB);
        Some(e)
    }

    /// Wait up to `ms` for an event `wanted` accepts. Others are dropped: a port
    /// status change that arrives while a command runs needs no answer here.
    fn wait_event(&mut self, ms: u64, wanted: impl Fn(&Trb) -> bool) -> Option<Trb> {
        let mut found = None;
        wait_for(ms, || {
            while let Some(e) = self.next_event() {
                if wanted(&e) {
                    found = Some(e);
                    return true;
                }
            }
            false
        });
        found
    }

    /// Run one command and return its completion event, if it succeeded.
    fn command(&mut self, t: Trb, what: &'static str) -> Result<Trb, Failure> {
        let addr = self.commands.push(t);
        self.doorbell(0, 0);
        let e = self
            .wait_event(500, |e| {
                trb::trb_type(e.control) == trb::TYPE_COMMAND_COMPLETION && e.param == addr
            })
            .ok_or(Failure::Step(what))?;
        if trb::completion_code(e.status) != trb::CC_SUCCESS {
            return Err(Failure::Step(what));
        }
        Ok(e)
    }

    /// Reset a USB2 root port, or leave a USB3 one (which enables itself), and return
    /// its speed once it is enabled.
    fn enable_port(&self, port: u8) -> Option<u8> {
        let at = self.portsc(port);
        let sc = self.regs.read32(at);
        if sc & PORT_CONNECTED == 0 {
            return None;
        }
        if sc & PORT_ENABLED == 0 {
            self.regs.write32(at, (sc & PORT_PRESERVE) | PORT_RESET);
            if !wait_for(500, || self.regs.read32(at) & PORT_RESET_CHANGE != 0) {
                return None;
            }
            // Ten milliseconds of reset recovery (USB 2.0, 7.1.7.5).
            sleep_ms(10);
        }
        let sc = self.regs.read32(at);
        self.regs.write32(at, (sc & PORT_PRESERVE) | (sc & PORT_CHANGE_BITS));
        if sc & PORT_ENABLED == 0 {
            return None;
        }
        Some(((sc >> 10) & 0xF) as u8)
    }

    /// Reset every connected root port and try each device in turn.
    fn find_keyboard(mut self) -> Result<Keyboard, Failure> {
        // Power, where the controller lets software switch it, and the 100 ms a newly
        // connected device is given to settle (USB 2.0, 7.1.7.3).
        for port in 1..=self.info.ports {
            let at = self.portsc(port);
            let sc = self.regs.read32(at);
            if sc & PORT_POWER == 0 {
                self.regs.write32(at, (sc & PORT_PRESERVE) | PORT_POWER);
            }
        }
        sleep_ms(100);

        let mut last = Failure::NoKeyboard;
        for port in 1..=self.info.ports {
            let Some(speed) = self.enable_port(port) else { continue };
            match self.try_device(port, speed) {
                Ok(found) => return Keyboard::start(self, found),
                Err(f) => last = f,
            }
        }
        Err(last)
    }

    /// Address the device on `port` and, if it is a boot keyboard, configure it.
    fn try_device(&mut self, port: u8, speed: u8) -> Result<FoundKeyboard, Failure> {
        let done = self.command(trb::enable_slot(), "enable slot")?;
        let slot = trb::slot_id(done.control);
        if slot == 0 || slot > self.info.slots {
            return Err(Failure::Step("enable slot"));
        }

        let output = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
        let input = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
        let buffer = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
        let ep0 = Ring::new().ok_or(Failure::Step("out of memory"))?;
        self.dcbaa.write64(usize::from(slot) * 8, output.phys());

        let mut dev = Device { slot, port, speed, input, _output: output, buffer, ep0 };
        let ctx = usize::from(self.info.context_size);

        // Address Device: the slot context and endpoint 0.
        let mut mps0 = trb::default_max_packet_0(speed);
        dev.write_input(ctx, 0b11, &trb::slot_context(speed, 1, port), 1, &dev.ep0_context(mps0));
        self.command(trb::address_device(dev.input.phys(), slot), "address device")?;

        // The first eight bytes of the device descriptor hold endpoint 0's real
        // packet size. If it differs from the guess, tell the controller.
        self.control(&mut dev, SetupPacket::get_descriptor(hid::DESC_DEVICE, 8))
            .map_err(|_| Failure::Step("device descriptor"))?;
        let stated = trb::max_packet_0_from_descriptor(speed, dev.buffer.read8(7));
        if stated != 0 && stated != mps0 {
            mps0 = stated;
            dev.write_input(ctx, 0b10, &[0; 4], 1, &dev.ep0_context(mps0));
            self.command(trb::evaluate_context(dev.input.phys(), slot), "evaluate context")?;
        }

        // The configuration descriptor: nine bytes for its total length, then all of it.
        self.control(&mut dev, SetupPacket::get_descriptor(hid::DESC_CONFIGURATION, 9))
            .map_err(|_| Failure::Step("configuration descriptor"))?;
        let total = u16::from(dev.buffer.read8(2)) | (u16::from(dev.buffer.read8(3)) << 8);
        let total = total.clamp(9, 512);
        self.control(&mut dev, SetupPacket::get_descriptor(hid::DESC_CONFIGURATION, total))
            .map_err(|_| Failure::Step("configuration descriptor"))?;
        let mut config = [0u8; 512];
        dev.buffer.read_bytes(0, &mut config[..usize::from(total)]);
        let Some(kbd) = hid::find_boot_keyboard(&config[..usize::from(total)]) else {
            return Err(Failure::NoKeyboard);
        };
        Ok(FoundKeyboard { dev, iface: kbd })
    }

    /// One control transfer on endpoint 0, with its data (if any) in the device's
    /// buffer page.
    fn control(&mut self, dev: &mut Device, setup: SetupPacket) -> Result<(), u8> {
        let dir_in = setup.request_type & 0x80 != 0;
        dev.ep0.push(trb::setup_stage(&setup));
        if setup.length > 0 {
            dev.ep0.push(trb::data_stage(dev.buffer.phys(), setup.length, dir_in));
        }
        dev.ep0.push(trb::status_stage(setup.length > 0 && dir_in));
        self.doorbell(dev.slot, 1);
        let slot = dev.slot;
        let e = self
            .wait_event(500, |e| {
                trb::trb_type(e.control) == trb::TYPE_TRANSFER_EVENT
                    && trb::slot_id(e.control) == slot
                    && trb::endpoint_id(e.control) == 1
            })
            .ok_or(0u8)?;
        match trb::completion_code(e.status) {
            trb::CC_SUCCESS | trb::CC_SHORT_PACKET => Ok(()),
            code => Err(code),
        }
    }
}

/// A device being enumerated: its slot and the pages that belong to it.
struct Device {
    slot: u8,
    port: u8,
    speed: u8,
    input: DmaPage,
    /// The output device context. The controller writes it; nothing here reads it,
    /// but it must stay allocated for as long as the slot exists.
    _output: DmaPage,
    buffer: DmaPage,
    ep0: Ring,
}

/// A device that turned out to be a keyboard, with what its descriptor said.
struct FoundKeyboard {
    dev: Device,
    iface: KeyboardInterface,
}

impl Device {
    fn ep0_context(&self, mps0: u16) -> [u32; 5] {
        trb::endpoint_context(trb::EP_TYPE_CONTROL, mps0, 0, self.ep0.phys(), 8, 0)
    }

    /// Fill the input context: the add flags, a slot context, and one endpoint context
    /// at device context index `dci`.
    fn write_input(&self, ctx: usize, add: u32, slot: &[u32; 4], dci: u8, ep: &[u32; 5]) {
        self.input.clear();
        self.input.write32(4, add);
        if add & 1 != 0 {
            for (i, word) in slot.iter().enumerate() {
                self.input.write32(ctx + i * 4, *word);
            }
        }
        let ep_at = ctx * (1 + usize::from(dci));
        for (i, word) in ep.iter().enumerate() {
            self.input.write32(ep_at + i * 4, *word);
        }
    }
}

impl Keyboard {
    /// Configure the found keyboard's endpoint and start reading it.
    fn start(mut hc: Controller, found: FoundKeyboard) -> Result<Self, Failure> {
        let FoundKeyboard { mut dev, iface } = found;
        let dci = trb::dci_for_in_endpoint(iface.endpoint);
        let ring = Ring::new().ok_or(Failure::Step("out of memory"))?;
        let report = DmaPage::alloc().ok_or(Failure::Step("out of memory"))?;
        let ctx = usize::from(hc.info.context_size);
        let max_packet = iface.max_packet.clamp(1, 64);
        let ep = trb::endpoint_context(
            trb::EP_TYPE_INTERRUPT_IN,
            max_packet,
            trb::interrupt_interval(dev.speed, iface.interval),
            ring.phys(),
            max_packet,
            max_packet,
        );
        let slot = trb::slot_context(dev.speed, dci, dev.port);
        dev.write_input(ctx, 1 | (1 << dci), &slot, dci, &ep);
        hc.command(trb::configure_endpoint(dev.input.phys(), dev.slot), "configure endpoint")?;
        hc.control(&mut dev, SetupPacket::set_configuration(iface.configuration))
            .map_err(|_| Failure::Step("set configuration"))?;
        // Both optional. A keyboard that refuses SET_PROTOCOL is very likely already
        // sending boot-shaped reports (most do in report protocol too), and one that
        // refuses SET_IDLE only repeats its report now and then, which the translator
        // ignores. SET_IDLE goes last because a refusal stalls endpoint 0, and nothing
        // after it uses endpoint 0.
        let _ = hc.control(&mut dev, SetupPacket::set_boot_protocol(iface.interface));
        let _ = hc.control(&mut dev, SetupPacket::set_idle_forever(iface.interface));

        let mut kbd = Self {
            hc,
            slot: dev.slot,
            dci,
            ring,
            report,
            max_packet,
            translator: BootReportTranslator::new(),
            queue: ScancodeQueue::new(),
            errors: 0,
            port: dev.port,
            speed: dev.speed,
            endpoint: iface.endpoint,
        };
        kbd.queue_read();
        Ok(kbd)
    }

    /// Errors after which the keyboard stops being read.
    const GIVE_UP: u32 = 16;

    fn queue_read(&mut self) {
        self.ring.push(trb::normal(self.report.phys(), u32::from(self.max_packet)));
        self.hc.doorbell(self.slot, self.dci);
    }

    /// Take whatever reports have arrived and turn them into scancodes.
    pub fn poll(&mut self) {
        if self.errors >= Self::GIVE_UP {
            return;
        }
        // Bounded, so a controller that floods events cannot hold the halt loop.
        for _ in 0..16 {
            let Some(e) = self.hc.next_event() else { return };
            if trb::trb_type(e.control) != trb::TYPE_TRANSFER_EVENT
                || trb::slot_id(e.control) != self.slot
                || trb::endpoint_id(e.control) != self.dci
            {
                continue;
            }
            match trb::completion_code(e.status) {
                trb::CC_SUCCESS | trb::CC_SHORT_PACKET => {
                    let asked = u32::from(self.max_packet);
                    let got = asked.saturating_sub(trb::residual_length(e.status)).min(8);
                    let mut report = [0u8; 8];
                    self.report.read_bytes(0, &mut report[..got as usize]);
                    self.translator.feed(&report[..got as usize], &mut self.queue);
                }
                _ => self.errors += 1,
            }
            if self.errors < Self::GIVE_UP {
                self.queue_read();
            }
        }
    }

    /// The next scancode, polling the controller first.
    pub fn scancode(&mut self) -> Option<u8> {
        if self.queue.is_empty() {
            self.poll();
        }
        self.queue.pop()
    }
}
