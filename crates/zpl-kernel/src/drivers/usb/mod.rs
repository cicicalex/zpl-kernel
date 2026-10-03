//! USB keyboard, for machines that have no PS/2 controller.
//!
//! Many current machines -- UEFI-only mini PCs especially -- have no i8042 at all, not
//! even an emulated one, so `kbd`, which reads ports `0x60`/`0x64`, never sees a key
//! there. This is the other way in: an xHCI controller found on the PCI bus, one boot
//! keyboard on one of its root ports, read by polling from the halt loop. Its reports
//! come out as scancode set 1, the bytes `kbd` already speaks, so the command line's
//! decoder is the same for both keyboards.
//!
//! # When it is used
//!
//! - **No i8042:** the USB keyboard is brought up. This is the case it exists for.
//! - **i8042 present, firmware holds the xHCI:** left alone. The firmware is very likely
//!   emulating a PS/2 keyboard from the USB one, which is how the keyboard works on
//!   such a machine today; taking the controller would end that.
//! - **i8042 present, firmware does not hold the xHCI:** brought up *as well*. Nothing
//!   else is serving a USB keyboard there, and the PS/2 path is not touched.
//! - **No xHCI:** nothing happens and nothing is printed, so a machine without one
//!   boots exactly as before.
//!
//! # Layout, and where the `unsafe` is
//!
//! - [`hid`], [`trb`]: plain data -- report translation, descriptor parsing, and the
//!   bit layouts of xHCI structures. No `unsafe`; tested on the host.
//! - `mmio`: the controller's register window. Mapping it is `unsafe`; reads and writes
//!   after that are bounds-checked and safe.
//! - `dma`: pages the controller reads and writes. Allocation and every access are
//!   bounds-checked and safe to call; the `unsafe` stays inside.
//! - `xhci`: the controller and the keyboard, built on those two. Its only `unsafe` is
//!   handing it a device: mapping the BAR and switching on bus mastering.
//! - this file: the one place the keyboard is kept, see [`init`].
//!
//! # Safety
//!
//! The keyboard lives in a `static mut`, the same way the shell keeps its line editor:
//! exactly one reader exists, the halt loop on the bootstrap CPU, and nothing re-enters
//! it. [`init`] and [`poll_scancode`] are only called from there.

pub mod hid;
pub mod trb;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod dma;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod mmio;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
mod xhci;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static mut KEYBOARD: Option<xhci::Keyboard> = None;

/// A short name for a port speed, for the report line.
#[must_use]
pub fn speed_name(speed: u8) -> &'static [u8] {
    match speed {
        trb::SPEED_FULL => b"full",
        trb::SPEED_LOW => b"low",
        trb::SPEED_HIGH => b"high",
        trb::SPEED_SUPER => b"super",
        _ => b"unknown",
    }
}

/// Look for xHCI controllers and bring up the first USB boot keyboard found on one.
///
/// `ps2_present` is whether the machine has an i8042; see the module header for what
/// it changes. Each line `say` is given is one line of the report, without its newline;
/// on a machine with no xHCI there are none. Returns whether a keyboard is now being
/// read.
///
/// Call once, from the halt loop, before the first [`poll_scancode`].
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn init(ps2_present: bool, say: &mut dyn FnMut(&[u8])) -> bool {
    use crate::drivers::console::MarkerLine;

    // The diagnostic image that skips the PCI walk skips this too: it exists to ask
    // whether touching the bus is what upsets the firmware.
    if cfg!(feature = "skip_pci") {
        return false;
    }
    let scan = crate::drivers::pci::scan();
    for dev in scan.devices.iter().flatten().filter(|d| crate::drivers::pci::is_xhci(d)) {
        let mut line = MarkerLine::new();
        line.push(b"[ZPL-USB] xhci ");
        push_bdf(&mut line, dev);
        line.push(b": ");
        match xhci::bring_up(dev, ps2_present) {
            Ok((kbd, info)) => {
                line.push(b"ports=");
                line.push_dec(u64::from(info.ports));
                line.push(b" slots=");
                line.push_dec(u64::from(info.slots));
                line.push(b" scratchpads=");
                line.push_dec(u64::from(info.scratchpads));
                line.push(b" context=");
                line.push_dec(u64::from(info.context_size));
                say(line.as_slice());
                let mut line = MarkerLine::new();
                line.push(b"[ZPL-USB] keyboard on port ");
                line.push_dec(u64::from(kbd.port));
                line.push(b", ");
                line.push(speed_name(kbd.speed));
                line.push(b" speed, endpoint ");
                line.push_dec(u64::from(kbd.endpoint & 0x0F));
                line.push(b" IN, boot protocol, polled");
                say(line.as_slice());
                // SAFETY: the single reader, see the module header.
                unsafe { *core::ptr::addr_of_mut!(KEYBOARD) = Some(kbd) };
                return true;
            }
            Err(xhci::Failure::LeftToFirmware) => {
                line.push(b"left to the firmware (it holds the controller, and a PS/2 port exists)");
            }
            Err(xhci::Failure::NoKeyboard) => line.push(b"no boot keyboard on a root port"),
            Err(xhci::Failure::Step(what)) => {
                line.push(b"stopped at: ");
                line.push(what.as_bytes());
            }
        }
        say(line.as_slice());
    }
    false
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn push_bdf(line: &mut crate::drivers::console::MarkerLine, dev: &crate::drivers::pci::PciDevice) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hex2 = |v: u8| [HEX[usize::from(v >> 4)], HEX[usize::from(v & 0xF)]];
    line.push(&hex2(dev.bus));
    line.push(b":");
    line.push(&hex2(dev.device));
    line.push(b".");
    line.push(&[HEX[usize::from(dev.function & 0x7)]]);
}

/// The next set-1 scancode from the USB keyboard, if one is up and has sent anything.
/// Never blocks.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn poll_scancode() -> Option<u8> {
    // SAFETY: the single reader, see the module header.
    let kbd = unsafe { &mut *core::ptr::addr_of_mut!(KEYBOARD) };
    kbd.as_mut()?.scancode()
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
pub fn poll_scancode() -> Option<u8> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_speed_the_ports_report_has_a_name() {
        for s in [trb::SPEED_FULL, trb::SPEED_LOW, trb::SPEED_HIGH, trb::SPEED_SUPER] {
            assert_ne!(speed_name(s), b"unknown");
        }
        assert_eq!(speed_name(0), b"unknown");
    }
}
