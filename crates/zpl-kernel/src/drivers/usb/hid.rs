//! A USB keyboard's boot report, turned into the scancodes the PS/2 path already speaks.
//!
//! **Why scancodes and not characters.** The command line already has a decoder that
//! turns scancode set 1 into ASCII, with Shift and Caps Lock, and it is tested. A USB
//! keyboard that speaks the same bytes reuses all of that: one table here, from HID
//! usage to set-1 make code, and the shell cannot tell which keyboard a key came from.
//!
//! **What a boot report is.** Eight bytes: a bitmap of the eight modifier keys, a
//! reserved byte, and up to six key usages that are down right now. It is the state of
//! the keyboard, not a list of events, so the events -- this key went down, that one
//! came up -- are the difference between one report and the one before it.
//!
//! Everything here is plain data handling, with no port and no register, so the tests
//! at the bottom run on the host. The configuration-descriptor walk that finds the
//! keyboard's interface lives here for the same reason.

/// Set-1 make code for a HID usage on the keyboard page, or `0` for "no key".
///
/// Bit 8 (`EXT`) marks a key that sends the `0xE0` prefix first: the arrows, the
/// navigation block, keypad Enter and keypad `/`, the right-hand modifiers.
const EXT: u16 = 0x100;

/// HID usage (index) to set-1 make code. Usages `0x00..=0x03` are not keys
/// (`0x01` is the "too many keys" marker, handled before this table is read).
/// Print Screen (`0x46`) and Pause (`0x48`) send multi-byte sequences no line editor
/// wants, and are left out.
const USAGE_TO_SET1: [u16; 0x66] = [
    // 0x00..0x03: reserved, ErrorRollOver, POSTFail, ErrorUndefined
    0, 0, 0, 0,
    // 0x04..0x1D: a..z
    0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32,
    0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    // 0x1E..0x27: 1..9, 0
    0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
    // 0x28 Enter, 0x29 Esc, 0x2A Backspace, 0x2B Tab, 0x2C Space
    0x1C, 0x01, 0x0E, 0x0F, 0x39,
    // 0x2D - , 0x2E = , 0x2F [ , 0x30 ] , 0x31 \ , 0x32 non-US # , 0x33 ; , 0x34 ' ,
    // 0x35 ` , 0x36 , , 0x37 . , 0x38 /
    0x0C, 0x0D, 0x1A, 0x1B, 0x2B, 0x2B, 0x27, 0x28, 0x29, 0x33, 0x34, 0x35,
    // 0x39 Caps Lock
    0x3A,
    // 0x3A..0x45: F1..F12
    0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x44, 0x57, 0x58,
    // 0x46 PrintScreen (left out), 0x47 Scroll Lock, 0x48 Pause (left out)
    0, 0x46, 0,
    // 0x49 Insert, Home, PgUp, Delete, End, PgDn, Right, Left, Down, Up
    EXT | 0x52, EXT | 0x47, EXT | 0x49, EXT | 0x53, EXT | 0x4F, EXT | 0x51,
    EXT | 0x4D, EXT | 0x4B, EXT | 0x50, EXT | 0x48,
    // 0x53 Num Lock, KP /, KP *, KP -, KP +, KP Enter
    0x45, EXT | 0x35, 0x37, 0x4A, 0x4E, EXT | 0x1C,
    // 0x59..0x63: KP 1..9, KP 0, KP .
    0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49, 0x52, 0x53,
    // 0x64 non-US \ , 0x65 Application
    0x56, EXT | 0x5D,
];

/// The eight modifier bits of byte 0, in bit order, as set-1 make codes.
const MODIFIER_TO_SET1: [u16; 8] = [
    0x1D,       // left Ctrl
    0x2A,       // left Shift
    0x38,       // left Alt
    EXT | 0x5B, // left GUI
    EXT | 0x1D, // right Ctrl
    0x36,       // right Shift
    EXT | 0x38, // right Alt
    EXT | 0x5C, // right GUI
];

/// Usage `0x01`: the keyboard is reporting more keys than fit in six slots, and every
/// slot holds this instead of a key. It says nothing about which keys are down.
const ERROR_ROLLOVER: u8 = 0x01;

/// The set-1 break bit, as in `kbd`.
const BREAK_BIT: u8 = 0x80;

/// The set-1 make code for a usage, with the `EXT` flag, or `None` if it is not a key
/// this table knows.
#[must_use]
pub fn usage_to_set1(usage: u8) -> Option<u16> {
    match USAGE_TO_SET1.get(usage as usize) {
        Some(&code) if code != 0 => Some(code),
        _ => None,
    }
}

/// A small ring of scancode bytes, between the report that produced them and the
/// reader that wants them one at a time.
///
/// When it is full the newest bytes are dropped. A keyboard's worth of typing between
/// two polls of the halt loop is a handful of bytes; 128 is far more than that.
pub struct ScancodeQueue {
    buf: [u8; Self::CAPACITY],
    head: usize,
    len: usize,
}

impl ScancodeQueue {
    pub const CAPACITY: usize = 128;

    #[must_use]
    pub const fn new() -> Self {
        Self { buf: [0; Self::CAPACITY], head: 0, len: 0 }
    }

    fn push(&mut self, byte: u8) {
        if self.len < Self::CAPACITY {
            self.buf[(self.head + self.len) % Self::CAPACITY] = byte;
            self.len += 1;
        }
    }

    /// The bytes for one key going down or up: the prefix if it has one, then the code.
    ///
    /// Pushed only when both fit, so a full queue cannot leave a lone `0xE0` behind to
    /// change the meaning of whatever comes next.
    fn push_key(&mut self, code: u16, released: bool) {
        let extended = code & EXT != 0;
        let need = if extended { 2 } else { 1 };
        if Self::CAPACITY - self.len < need {
            return;
        }
        if extended {
            self.push(0xE0);
        }
        let make = (code & 0x7F) as u8;
        self.push(if released { make | BREAK_BIT } else { make });
    }

    /// The oldest byte, if any.
    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buf[self.head];
        self.head = (self.head + 1) % Self::CAPACITY;
        self.len -= 1;
        Some(byte)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for ScancodeQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Turns a stream of boot reports into set-1 make and break codes.
#[derive(Clone, Copy)]
pub struct BootReportTranslator {
    modifiers: u8,
    keys: [u8; 6],
}

impl BootReportTranslator {
    #[must_use]
    pub const fn new() -> Self {
        Self { modifiers: 0, keys: [0; 6] }
    }

    /// Compare one report with the last and queue what changed.
    ///
    /// The order matters to the decoder downstream, which only tracks Shift and Caps
    /// Lock: releases go first, then modifiers coming down, then keys coming down. A
    /// report that releases Shift and presses `a` at once therefore types `a`, and one
    /// that presses both types `A`.
    ///
    /// A report shorter than three bytes is ignored, as is one in the
    /// too-many-keys state, which carries no information about which keys are down.
    pub fn feed(&mut self, report: &[u8], out: &mut ScancodeQueue) {
        if report.len() < 3 {
            return;
        }
        let modifiers = report[0];
        let mut keys = [0u8; 6];
        for (slot, &usage) in keys.iter_mut().zip(report[2..].iter()) {
            *slot = usage;
        }
        if keys.contains(&ERROR_ROLLOVER) {
            return;
        }

        // Keys that were down and are not any more.
        for &old in self.keys.iter().filter(|&&k| k != 0) {
            if !keys.contains(&old) {
                if let Some(code) = usage_to_set1(old) {
                    out.push_key(code, true);
                }
            }
        }
        // Modifiers, released then pressed.
        for (bit, &code) in MODIFIER_TO_SET1.iter().enumerate() {
            let mask = 1u8 << bit;
            if self.modifiers & mask != 0 && modifiers & mask == 0 {
                out.push_key(code, true);
            }
        }
        for (bit, &code) in MODIFIER_TO_SET1.iter().enumerate() {
            let mask = 1u8 << bit;
            if self.modifiers & mask == 0 && modifiers & mask != 0 {
                out.push_key(code, false);
            }
        }
        // Keys that are down now and were not before.
        for &new in keys.iter().filter(|&&k| k != 0) {
            if !self.keys.contains(&new) {
                if let Some(code) = usage_to_set1(new) {
                    out.push_key(code, false);
                }
            }
        }

        self.modifiers = modifiers;
        self.keys = keys;
    }
}

impl Default for BootReportTranslator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Descriptors
// ---------------------------------------------------------------------------

/// Descriptor type codes, from the USB 2.0 specification, chapter 9.
pub const DESC_DEVICE: u8 = 1;
pub const DESC_CONFIGURATION: u8 = 2;
const DESC_INTERFACE: u8 = 4;
const DESC_ENDPOINT: u8 = 5;

/// Interface class 3 is HID; subclass 1 says it speaks the boot protocol; protocol 1
/// says the boot protocol it speaks is the keyboard's.
const CLASS_HID: u8 = 3;
const SUBCLASS_BOOT: u8 = 1;
const PROTOCOL_KEYBOARD: u8 = 1;

/// What the driver needs to know about a boot keyboard, read out of its
/// configuration descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardInterface {
    /// `bConfigurationValue`, for SET_CONFIGURATION.
    pub configuration: u8,
    /// `bInterfaceNumber`, for the class requests.
    pub interface: u8,
    /// `bEndpointAddress` of the interrupt IN endpoint, direction bit included.
    pub endpoint: u8,
    pub max_packet: u16,
    /// `bInterval`, in the units the device's speed gives it.
    pub interval: u8,
}

/// Walk a configuration descriptor (with everything that follows it) and find the
/// first boot keyboard interface and its interrupt IN endpoint.
///
/// A descriptor with a zero length would make the walk stand still, and one that runs
/// past the buffer would read what is not there; both end the walk instead.
#[must_use]
pub fn find_boot_keyboard(config: &[u8]) -> Option<KeyboardInterface> {
    if config.len() < 9 || config[1] != DESC_CONFIGURATION {
        return None;
    }
    let configuration = config[5];
    let mut at = 0usize;
    let mut in_keyboard: Option<u8> = None;
    while at + 2 <= config.len() {
        let len = config[at] as usize;
        let kind = config[at + 1];
        if len < 2 || at + len > config.len() {
            return None;
        }
        let d = &config[at..at + len];
        match kind {
            DESC_INTERFACE if len >= 9 => {
                in_keyboard = (d[5] == CLASS_HID
                    && d[6] == SUBCLASS_BOOT
                    && d[7] == PROTOCOL_KEYBOARD)
                    .then_some(d[2]);
            }
            DESC_ENDPOINT if len >= 7 => {
                // Bit 7 of the address is IN; bits 1:0 of the attributes are 3 for an
                // interrupt endpoint.
                if let Some(interface) = in_keyboard {
                    if d[2] & 0x80 != 0 && d[3] & 0x03 == 0x03 {
                        return Some(KeyboardInterface {
                            configuration,
                            interface,
                            endpoint: d[2],
                            max_packet: u16::from_le_bytes([d[4], d[5]]) & 0x07FF,
                            interval: d[6],
                        });
                    }
                }
            }
            _ => {}
        }
        at += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::kbd::Decoder;

    /// Run reports through the translator and then the shell's own decoder.
    fn typed(reports: &[[u8; 8]]) -> ([u8; 32], usize) {
        let mut tr = BootReportTranslator::new();
        let mut q = ScancodeQueue::new();
        let mut dec = Decoder::new();
        let mut out = [0u8; 32];
        let mut n = 0;
        for r in reports {
            tr.feed(r, &mut q);
            while let Some(code) = q.pop() {
                if let Some(ch) = dec.feed(code) {
                    out[n] = ch;
                    n += 1;
                }
            }
        }
        (out, n)
    }

    fn key(usage: u8) -> [u8; 8] {
        [0, 0, usage, 0, 0, 0, 0, 0]
    }
    const NONE: [u8; 8] = [0; 8];

    #[test]
    fn version_typed_on_a_usb_keyboard_reaches_the_decoder_as_version() {
        // v e r s i o n Enter, each pressed and released.
        let mut reports = [NONE; 16];
        for (i, &u) in [0x19u8, 0x08, 0x15, 0x16, 0x0C, 0x12, 0x11, 0x28].iter().enumerate() {
            reports[2 * i] = key(u);
        }
        let (out, n) = typed(&reports);
        assert_eq!(&out[..n], b"version\r");
    }

    #[test]
    fn shift_from_the_modifier_byte_gives_capitals_and_symbols() {
        let (out, n) = typed(&[
            [0x02, 0, 0x04, 0, 0, 0, 0, 0], // left Shift + a
            [0x02, 0, 0, 0, 0, 0, 0, 0],
            [0x20, 0, 0x1E, 0, 0, 0, 0, 0], // right Shift + 1
            NONE,
            key(0x04),
        ]);
        assert_eq!(&out[..n], b"A!a");
    }

    #[test]
    fn releasing_shift_and_pressing_a_key_in_one_report_types_it_unshifted() {
        let (out, n) = typed(&[[0x02, 0, 0, 0, 0, 0, 0, 0], key(0x04)]);
        assert_eq!(&out[..n], b"a");
    }

    #[test]
    fn a_key_held_across_reports_is_typed_once() {
        let (out, n) = typed(&[key(0x04), key(0x04), [0, 0, 0x04, 0x05, 0, 0, 0, 0]]);
        assert_eq!(&out[..n], b"ab");
    }

    #[test]
    fn rollover_reports_change_nothing() {
        let mut tr = BootReportTranslator::new();
        let mut q = ScancodeQueue::new();
        tr.feed(&key(0x04), &mut q);
        while q.pop().is_some() {}
        tr.feed(&[0, 0, 1, 1, 1, 1, 1, 1], &mut q);
        assert!(q.is_empty(), "no release is invented for the held key");
        tr.feed(&NONE, &mut q);
        assert_eq!(q.pop(), Some(0x1E | BREAK_BIT));
    }

    #[test]
    fn an_arrow_key_arrives_with_its_prefix_and_types_nothing() {
        let mut tr = BootReportTranslator::new();
        let mut q = ScancodeQueue::new();
        tr.feed(&key(0x52), &mut q); // Up
        tr.feed(&NONE, &mut q);
        let mut bytes = [0u8; 4];
        for b in &mut bytes {
            *b = q.pop().unwrap();
        }
        assert_eq!(bytes, [0xE0, 0x48, 0xE0, 0xC8]);
        let (_, n) = typed(&[key(0x52), NONE]);
        assert_eq!(n, 0);
    }

    #[test]
    fn enter_backspace_and_keypad_enter_are_the_control_codes_the_editor_wants() {
        let (out, n) = typed(&[key(0x28), NONE, key(0x2A), NONE, key(0x58), NONE]);
        assert_eq!(&out[..n], &[b'\r', 0x08, b'\r']);
    }

    #[test]
    fn every_digit_and_letter_matches_the_ps2_layout() {
        let mut reports = [NONE; 72];
        for i in 0..36u8 {
            reports[2 * i as usize] = key(0x04 + i);
        }
        let (out, n) = typed(&reports[..32]);
        assert_eq!(&out[..n], b"abcdefghijklmnop");
        let (out, n) = typed(&reports[32..72]);
        assert_eq!(&out[..n], b"qrstuvwxyz1234567890");
    }

    #[test]
    fn a_short_report_is_ignored() {
        let mut tr = BootReportTranslator::new();
        let mut q = ScancodeQueue::new();
        tr.feed(&[0x02, 0], &mut q);
        assert!(q.is_empty());
    }

    #[test]
    fn unknown_usages_past_the_table_produce_nothing() {
        assert_eq!(usage_to_set1(0x00), None);
        assert_eq!(usage_to_set1(0x46), None);
        assert_eq!(usage_to_set1(0x66), None);
        assert_eq!(usage_to_set1(0xFF), None);
    }

    #[test]
    fn a_full_queue_drops_whole_keys_never_half_of_one() {
        let mut q = ScancodeQueue::new();
        for _ in 0..ScancodeQueue::CAPACITY - 1 {
            q.push_key(0x1E, false);
        }
        q.push_key(EXT | 0x48, false); // needs two, one left
        let mut last = 0;
        while let Some(b) = q.pop() {
            last = b;
        }
        assert_eq!(last, 0x1E, "no lone 0xE0 at the end");
    }

    /// A configuration descriptor shaped like the one a boot keyboard (QEMU's `usb-kbd`
    /// among them) returns: configuration, interface, HID descriptor, endpoint.
    const QEMU_USB_KBD_CONFIG: [u8; 34] = [
        0x09, 0x02, 0x22, 0x00, 0x01, 0x01, 0x04, 0xA0, 0x32, // configuration
        0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x01, 0x00, // interface: HID boot kbd
        0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x3F, 0x00, // HID descriptor
        0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x07, // endpoint 1 IN, interrupt
    ];

    #[test]
    fn the_keyboard_interface_is_found_in_a_real_configuration() {
        assert_eq!(
            find_boot_keyboard(&QEMU_USB_KBD_CONFIG),
            Some(KeyboardInterface {
                configuration: 1,
                interface: 0,
                endpoint: 0x81,
                max_packet: 8,
                interval: 7,
            })
        );
    }

    #[test]
    fn a_mouse_is_not_a_keyboard() {
        let mut c = QEMU_USB_KBD_CONFIG;
        c[16] = 0x02; // protocol 2: mouse
        assert_eq!(find_boot_keyboard(&c), None);
    }

    #[test]
    fn the_keyboard_is_found_behind_another_interface() {
        // A composite device: a vendor interface with an IN endpoint first.
        let mut c = [0u8; 50];
        c[..9].copy_from_slice(&QEMU_USB_KBD_CONFIG[..9]);
        c[9..18].copy_from_slice(&[0x09, 0x04, 0x01, 0x00, 0x01, 0xFF, 0x00, 0x00, 0x00]);
        c[18..25].copy_from_slice(&[0x07, 0x05, 0x82, 0x03, 0x40, 0x00, 0x01]);
        c[25..50].copy_from_slice(&QEMU_USB_KBD_CONFIG[9..34]);
        let k = find_boot_keyboard(&c).expect("found");
        assert_eq!(k.endpoint, 0x81);
        assert_eq!(k.interface, 0);
    }

    #[test]
    fn a_broken_descriptor_ends_the_walk_instead_of_looping_or_overrunning() {
        let mut c = QEMU_USB_KBD_CONFIG;
        c[9] = 0; // zero length
        assert_eq!(find_boot_keyboard(&c), None);
        let mut c = QEMU_USB_KBD_CONFIG;
        c[27] = 40; // runs past the end
        assert_eq!(find_boot_keyboard(&c), None);
        assert_eq!(find_boot_keyboard(&[]), None);
    }
}
