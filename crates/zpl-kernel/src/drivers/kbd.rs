//! PS/2 (i8042) keyboard: the three-key menu reader, and a full scancode decoder.
//!
//! **Polling, not interrupts.** The kernel has no keyboard IRQ handler and both readers
//! run inside the halt loop, so polling the controller there costs nothing and leaves
//! interrupt routing and the scheduler untouched. Nothing else in the kernel reads ports
//! `0x60` / `0x64`, so there is no consumer to race with.
//!
//! Two readers, because they want different things:
//!
//! - [`poll_menu_key`] decodes the three digits the v0.4 menu uses and drops everything
//!   else, including all break codes.
//! - [`Decoder`] turns scancode set 1 into ASCII for the shell, tracking Shift and Caps
//!   Lock. It is a pure state machine over bytes -- no ports -- so it is tested on the
//!   host by feeding it the scancodes a real keyboard would send.

use core::sync::atomic::{AtomicBool, Ordering};

const PS2_DATA: u16 = 0x60;
const PS2_STATUS: u16 = 0x64;
/// Status bit 0: output buffer full, i.e. there is a byte to read from `0x60`.
const STATUS_OUTPUT_FULL: u8 = 1 << 0;
/// Status bit 5 marks a byte coming from the auxiliary device (mouse), not the keyboard.
const STATUS_AUX: u8 = 1 << 5;

/// Scancode set 1 make codes for the three menu keys.
const MAKE_1: u8 = 0x02;
const MAKE_2: u8 = 0x03;
const MAKE_3: u8 = 0x04;

static WARNED_EMPTY: AtomicBool = AtomicBool::new(false);

/// One of the keys the v0.4 menu reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// `1` — run the clean workload.
    One,
    /// `2` — run the hostile workload.
    Two,
    /// `3` — show and verify the audit chain.
    Three,
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
use crate::arch::port::inb;

/// On a host there are no ports, and the decoder below is what the tests exercise.
#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
#[inline]
unsafe fn inb(_port: u16) -> u8 {
    0
}

/// Drain whatever the controller has buffered and return the first menu key in it.
///
/// Never blocks: if no byte is waiting it returns `None` straight away, so the halt loop
/// keeps its own pace. The drain is bounded so a stuck controller that always reports
/// "byte ready" cannot wedge the loop.
pub fn poll_menu_key() -> Option<MenuKey> {
    let mut found = None;
    for _ in 0..64 {
        let status = unsafe { inb(PS2_STATUS) };
        if status & STATUS_OUTPUT_FULL == 0 {
            break;
        }
        let code = unsafe { inb(PS2_DATA) };
        if status & STATUS_AUX != 0 {
            continue; // mouse byte; drop it
        }
        if found.is_none() {
            found = match code {
                MAKE_1 => Some(MenuKey::One),
                MAKE_2 => Some(MenuKey::Two),
                MAKE_3 => Some(MenuKey::Three),
                _ => None,
            };
        }
    }
    let _ = WARNED_EMPTY.load(Ordering::Relaxed);
    found
}

// ---------------------------------------------------------------------------
// Full scancode decoder, for the shell
// ---------------------------------------------------------------------------

/// Scancode set 1, unshifted, indexed by make code. `0` means "no character".
///
/// Only the main block is filled in: the keypad, the function keys and everything
/// behind the `0xE0` prefix produce nothing, which is what a line editor wants. Byte
/// `0x1C` is Enter and `0x0E` is Backspace; both are given their ASCII control codes
/// so the editor needs no special cases for them.
const UNSHIFTED: [u8; 0x40] = [
    //  0x00  0x01=Esc
    0, 0, b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'[', b']', b'\r', 0, b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v',
    b'b', b'n', b'm', b',', b'.', b'/', 0, 0,
    0, b' ', 0, 0, 0, 0, 0, 0,
];

/// The same table with Shift held. Differs only where the US layout differs.
const SHIFTED: [u8; 0x40] = [
    0, 0, b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t',
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', b'\r', 0, b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b'<', b'>', b'?', 0, 0,
    0, b' ', 0, 0, 0, 0, 0, 0,
];

const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_CAPS: u8 = 0x3A;
/// Prefix byte for the extended keys (arrows, right Ctrl, and friends).
const SC_EXTENDED: u8 = 0xE0;
/// A break code is the make code with bit 7 set.
const BREAK_BIT: u8 = 0x80;

/// Scancode set 1 to ASCII, with Shift and Caps Lock.
///
/// Holds no lock and touches no port: feed it bytes, take characters out. The shell
/// owns one of these and the tests own several.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decoder {
    shift_left: bool,
    shift_right: bool,
    caps: bool,
    /// The last byte was `0xE0`, so this one belongs to an extended key.
    extended: bool,
}

impl Decoder {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shift_left: false,
            shift_right: false,
            caps: false,
            extended: false,
        }
    }

    fn shift(&self) -> bool {
        self.shift_left || self.shift_right
    }

    /// Feed one scancode; get back an ASCII byte when the key produces one.
    ///
    /// Break codes, modifiers and everything after a `0xE0` prefix return `None`. An
    /// arrow key therefore yields nothing at all rather than a stray letter, which is
    /// exactly what the line editor wants from a key it cannot act on.
    pub fn feed(&mut self, code: u8) -> Option<u8> {
        if code == SC_EXTENDED {
            self.extended = true;
            return None;
        }
        let released = code & BREAK_BIT != 0;
        let make = code & !BREAK_BIT;

        if self.extended {
            // One extended key is worth decoding: the keypad Enter, which shares
            // `0x1C` with the main Enter. Everything else behind the prefix is
            // dropped, modifiers included, so a released right Shift cannot clear a
            // left Shift that is still down.
            self.extended = false;
            if make == 0x1C && !released {
                return Some(b'\r');
            }
            return None;
        }

        match make {
            SC_LSHIFT => {
                self.shift_left = !released;
                return None;
            }
            SC_RSHIFT => {
                self.shift_right = !released;
                return None;
            }
            SC_CAPS => {
                // Toggles on press, ignores the release.
                if !released {
                    self.caps = !self.caps;
                }
                return None;
            }
            _ => {}
        }

        if released {
            return None;
        }
        let index = make as usize;
        if index >= UNSHIFTED.len() {
            return None;
        }
        let ch = if self.shift() {
            SHIFTED[index]
        } else {
            UNSHIFTED[index]
        };
        if ch == 0 {
            return None;
        }
        // Caps Lock affects letters only, and inverts whatever Shift already did.
        if self.caps && ch.is_ascii_alphabetic() {
            return Some(if self.shift() {
                ch.to_ascii_lowercase()
            } else {
                ch.to_ascii_uppercase()
            });
        }
        Some(ch)
    }
}

/// Read one scancode from the controller, if one is waiting.
///
/// Never blocks. Auxiliary-device bytes (a mouse) are dropped here so the shell never
/// sees them.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn poll_scancode() -> Option<u8> {
    // Drain past the mouse, the way `poll_menu_key` always has.
    //
    // Status bit 5 marks a byte from the auxiliary device. This used to return `None`
    // on one, which ended the caller's drain loop in `crates/zpl-kernel/src/ui/shell.rs` -- and on a
    // the touchpad is a PS/2 mouse that talks constantly. The first byte waiting was
    // nearly always its, so the shell stopped before reaching anything the keyboard
    // had sent, every time. The prompt looked dead while the controller was full of
    // keystrokes.
    //
    // It never showed under QEMU `-machine pc`, because nothing moves the mouse there
    // and no auxiliary byte is ever queued. Three rounds of test images on real
    // hardware looked elsewhere for it.
    //
    // Bounded for the reason the menu reader is bounded: a controller that always
    // claims to have a byte must not be able to wedge the halt loop.
    for _ in 0..64 {
        let status = unsafe { inb(PS2_STATUS) };
        if status & STATUS_OUTPUT_FULL == 0 {
            return None;
        }
        let code = unsafe { inb(PS2_DATA) };
        if status & STATUS_AUX != 0 {
            continue; // mouse byte; drop it and keep looking
        }
        announce_first_scancode(code);
        note_scancode(code);
        return Some(code);
    }
    None
}

/// How many scancodes have been read, and the last one. Diagnostic only.
#[cfg(feature = "kbd_probe")]
static SCANCODES_SEEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "kbd_probe")]
static LAST_SCANCODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

#[cfg(feature = "kbd_probe")]
// Dead on the host, where its one caller is not compiled. Same reason as
// `first_scancode_line` above.
#[cfg_attr(not(all(target_os = "none", target_arch = "x86_64")), allow(dead_code))]
fn note_scancode(code: u8) {
    use core::sync::atomic::Ordering;
    SCANCODES_SEEN.fetch_add(1, Ordering::Relaxed);
    LAST_SCANCODE.store(code, Ordering::Relaxed);
}

#[cfg(not(feature = "kbd_probe"))]
// Dead on the host, where its one caller is not compiled. Same reason as
// `first_scancode_line` above.
#[cfg_attr(not(all(target_os = "none", target_arch = "x86_64")), allow(dead_code))]
fn note_scancode(_code: u8) {}

/// The controller's status byte, how many scancodes have been read, and the last one.
///
/// For the panel, which redraws twice a second, so the answer to "is anything arriving
/// at all" stays on screen instead of being one line that scrolls past. A photograph of
/// a stuck prompt then carries the answer.
///
/// **Reads `0x64` only.** The status port does not consume anything; reading `0x60`
/// would take the byte away from the shell, so a diagnostic that did that would break
/// the thing it is diagnosing.
#[cfg(all(feature = "kbd_probe", target_os = "none", target_arch = "x86_64"))]
pub fn probe_snapshot() -> (u8, u32, u8) {
    use core::sync::atomic::Ordering;
    let status = unsafe { inb(PS2_STATUS) };
    (
        status,
        SCANCODES_SEEN.load(Ordering::Relaxed),
        LAST_SCANCODE.load(Ordering::Relaxed),
    )
}

#[cfg(all(feature = "kbd_probe", not(all(target_os = "none", target_arch = "x86_64"))))]
pub fn probe_snapshot() -> (u8, u32, u8) {
    (0, 0, 0)
}

/// Say, once, that a scancode reached the kernel at all.
///
/// This exists to make the next test on real hardware decide something instead of
/// raising another question. When a prompt will not take input there are two very
/// different faults with one symptom: the controller is delivering nothing, or it is
/// delivering and something above drops it. The line below separates them without a
/// second trip to the machine.
///
/// It matters here because the keyboard is a USB one reaching the kernel through the
/// firmware's legacy PS/2 emulation, which is a path the kernel can neither see nor
/// control -- so "did a byte arrive" is the only question worth asking first.
///
/// Printed on the first scancode only, which is necessarily after boot, so the marker
/// sequence the determinism gate hashes is untouched.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn announce_first_scancode(code: u8) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static ANNOUNCED: AtomicBool = AtomicBool::new(false);

    if ANNOUNCED.swap(true, Ordering::Relaxed) {
        return;
    }
    crate::drivers::console::emit_critical_marker(&first_scancode_line(code));
}

/// The line [`announce_first_scancode`] prints, built separately so the host can check
/// it. Writing the two hex digits means indexing into a fixed template, and an index
/// worked out by hand is exactly the kind of thing that silently lands in the middle of
/// a word -- which it did, the first time this was written.
// Dead only on the host, where its one caller is not compiled. The template still has
// to be checked somewhere, and the host is the only place the tests run.
#[cfg_attr(not(all(target_os = "none", target_arch = "x86_64")), allow(dead_code))]
pub(crate) fn first_scancode_line(code: u8) -> [u8; 46] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    /// Index of the first `_` in the template below.
    const DIGITS_AT: usize = 27;

    let mut line = *b"[ZPL-KBD] first scancode 0x__ via ps2 polling\n";
    line[DIGITS_AT] = HEX[(code >> 4) as usize];
    line[DIGITS_AT + 1] = HEX[(code & 0x0F) as usize];
    line
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
pub fn poll_scancode() -> Option<u8> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- the full decoder --------------------------------------------------

    /// Type a word the way a keyboard would: make code, then break code.
    fn type_word(dec: &mut Decoder, codes: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        let mut n = 0;
        for &code in codes {
            if let Some(ch) = dec.feed(code) {
                if n < out.len() {
                    out[n] = ch;
                    n += 1;
                }
            }
            // The release of the same key must produce nothing.
            assert_eq!(dec.feed(code | BREAK_BIT), None);
        }
        out
    }

    #[test]
    fn letters_come_out_lowercase_by_default() {
        let mut dec = Decoder::new();
        // v e r s i o n
        let out = type_word(&mut dec, &[0x2F, 0x12, 0x13, 0x1F, 0x17, 0x18, 0x31]);
        assert_eq!(&out[..7], b"version");
    }

    #[test]
    fn shift_held_gives_capitals_and_the_symbols_above_the_digits() {
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(SC_LSHIFT), None);
        assert_eq!(dec.feed(0x1E), Some(b'A'));
        assert_eq!(dec.feed(0x02), Some(b'!'));
        assert_eq!(dec.feed(SC_LSHIFT | BREAK_BIT), None);
        assert_eq!(dec.feed(0x1E), Some(b'a'));
        assert_eq!(dec.feed(0x02), Some(b'1'));
    }

    #[test]
    fn caps_lock_affects_letters_and_not_digits() {
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(SC_CAPS), None);
        assert_eq!(dec.feed(SC_CAPS | BREAK_BIT), None);
        assert_eq!(dec.feed(0x1E), Some(b'A'));
        assert_eq!(dec.feed(0x02), Some(b'1'), "caps lock is not shift");
        // Shift with caps on gives lowercase back.
        assert_eq!(dec.feed(SC_LSHIFT), None);
        assert_eq!(dec.feed(0x1E), Some(b'a'));
    }

    #[test]
    fn caps_lock_toggles_on_press_only() {
        let mut dec = Decoder::new();
        dec.feed(SC_CAPS);
        dec.feed(SC_CAPS | BREAK_BIT);
        assert_eq!(dec.feed(0x1E), Some(b'A'));
        dec.feed(SC_CAPS);
        dec.feed(SC_CAPS | BREAK_BIT);
        assert_eq!(dec.feed(0x1E), Some(b'a'));
    }

    #[test]
    fn enter_backspace_space_and_tab_arrive_as_their_control_codes() {
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(0x1C), Some(b'\r'));
        assert_eq!(dec.feed(0x0E), Some(0x08));
        assert_eq!(dec.feed(0x39), Some(b' '));
        assert_eq!(dec.feed(0x0F), Some(b'\t'));
    }

    #[test]
    fn an_arrow_key_produces_nothing_at_all() {
        // Up arrow is E0 48, released E0 C8. None of the four bytes is a character,
        // and in particular 0x48 alone would otherwise decode as `i`.
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(SC_EXTENDED), None);
        assert_eq!(dec.feed(0x48), None);
        assert_eq!(dec.feed(SC_EXTENDED), None);
        assert_eq!(dec.feed(0xC8), None);
        // The next ordinary key still works.
        assert_eq!(dec.feed(0x1E), Some(b'a'));
    }

    #[test]
    fn the_keypad_enter_submits_like_the_main_one() {
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(SC_EXTENDED), None);
        assert_eq!(dec.feed(0x1C), Some(b'\r'));
    }

    #[test]
    fn a_released_right_shift_does_not_clear_a_held_left_shift() {
        let mut dec = Decoder::new();
        dec.feed(SC_LSHIFT);
        dec.feed(SC_RSHIFT);
        dec.feed(SC_RSHIFT | BREAK_BIT);
        assert_eq!(dec.feed(0x1E), Some(b'A'), "left shift is still down");
    }

    #[test]
    fn escape_and_the_function_keys_produce_nothing() {
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(0x01), None, "Escape");
        for f in 0x3B..=0x3F {
            assert_eq!(dec.feed(f), None, "F key {f:#x}");
        }
    }

    #[test]
    fn a_scancode_past_the_table_is_dropped_instead_of_indexing_out_of_bounds() {
        let mut dec = Decoder::new();
        for code in 0x40u8..0x7F {
            // No panic, and nothing that is not in the table comes out.
            let _ = dec.feed(code);
        }
    }

    #[test]
    fn the_two_tables_are_the_same_length_and_agree_where_they_should() {
        assert_eq!(UNSHIFTED.len(), SHIFTED.len());
        for i in 0..UNSHIFTED.len() {
            // A key that produces nothing unshifted must produce nothing shifted.
            assert_eq!(
                UNSHIFTED[i] == 0,
                SHIFTED[i] == 0,
                "tables disagree about whether {i:#x} is a key"
            );
        }
    }

    #[test]
    fn the_menu_digits_decode_the_same_way_in_both_readers() {
        // The v0.4 menu and the shell must agree about what `1`, `2`, `3` are, or a
        // demo and the prompt would disagree about which key was pressed.
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(MAKE_1), Some(b'1'));
        assert_eq!(dec.feed(MAKE_2), Some(b'2'));
        assert_eq!(dec.feed(MAKE_3), Some(b'3'));
    }

    #[test]
    fn menu_keys_are_distinct() {
        assert_ne!(MenuKey::One, MenuKey::Two);
        assert_ne!(MenuKey::Two, MenuKey::Three);
        assert_ne!(MenuKey::One, MenuKey::Three);
    }

    #[test]
    fn make_codes_are_the_set1_digits() {
        // Scancode set 1: `1`, `2`, `3` sit at 0x02..0x04, right after Escape (0x01).
        assert_eq!(MAKE_1, 0x02);
        assert_eq!(MAKE_2, 0x03);
        assert_eq!(MAKE_3, 0x04);
        assert_eq!(MAKE_2, MAKE_1 + 1);
        assert_eq!(MAKE_3, MAKE_2 + 1);
    }

    #[test]
    fn the_first_scancode_line_puts_the_hex_digits_where_it_says() {
        let line = first_scancode_line(0x1E);
        let text = core::str::from_utf8(&line).expect("ascii");
        assert_eq!(text, "[ZPL-KBD] first scancode 0x1e via ps2 polling
");
        let line = first_scancode_line(0x00);
        assert!(core::str::from_utf8(&line).unwrap().contains("0x00 via"));
        let line = first_scancode_line(0xFF);
        assert!(core::str::from_utf8(&line).unwrap().contains("0xff via"));
    }

    #[test]
    fn poll_is_none_without_hardware() {
        // On the host shim `inb` returns 0, so the output-full bit is clear.
        assert_eq!(poll_menu_key(), None);
    }
}
