//! Bus-level drivers (virtio, PCI helpers). Task #20 virtio-net lands here.

/// PCI configuration space and bus enumeration. The classification half builds
/// everywhere so it can be tested on a host; the port I/O inside it is gated.
pub mod pci;

/// Intel e1000, as far as reading the card's own MAC address. Same split as
/// `pci`: the decoding builds everywhere, the MMIO does not.
pub mod e1000;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod virtio_net;

pub mod serial;

/// PS/2 keyboard: the v0.4 three-key reader and the shell's full scancode decoder.
///
/// Declared unconditionally so the decoder's state machine is compiled and tested by a
/// plain `cargo test`. The port reads inside it already have a host shim; the menu-only
/// items stay gated on `v04_menu` within the module.
pub mod kbd;

// The module works on both boot paths: `vga_buffer_base` already falls back to
// the bare identity-mapped 0xB8000 when there is no HHDM offset, which is the
// `-kernel` case. It used to be gated out of `qemu_boot` builds to keep that
// binary byte-identical; the cost was that a plain `-kernel` boot could not put
// anything on a screen at all.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "vga_crit_mirror"
))]
pub mod vga;

/// v0.4 framebuffer console: what `vga` does for 0xB8000, this does for the graphical
/// framebuffer Limine actually hands us on real hardware.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    not(feature = "qemu_boot"),
    feature = "fb_crit_mirror"
))]
pub mod fbcon;

/// The one place a marker is written to every surface this boot path has.
pub mod console;
