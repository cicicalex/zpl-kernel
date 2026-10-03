//! A fixed panel at the bottom of the screen showing what the policy gate is doing.
//!
//! The boot log scrolls past faster than anyone can read, and the one thing a
//! visitor wants to know — *is the gate actually deciding anything?* — was only
//! visible by catching a line mid-scroll. The panel owns the bottom
//! [`crate::drivers::vga::PANEL_ROWS`] rows, which the log no longer scrolls, and keeps a
//! running count plus the last three decisions with the rule that produced each.
//!
//! Drawn on whichever console the build has. The `-kernel` path has the VGA text
//! buffer at `0xB8000`; the Limine path has a graphical framebuffer and no usable
//! text buffer, so the same rows are painted as 8x8 glyphs instead. Both at once
//! when both mirrors are on.
//!
//! The bottom row carries what used to be a separate three-row summary underneath
//! the panel. Two blocks competing for the bottom of the screen meant the panel was
//! simply never drawn on the ISO, and a machine booted from a stick showed the
//! summary and no panel at all. One block, one place, one relayout.
//!
//! **Nothing here writes to COM1.** The serial log, and therefore every hash and
//! gate derived from it, is exactly what it was before the panel existed.

// Bare metal, with or without a surface to draw on: `sched_marker::record` names `Site`
// and `Who`, and every decision goes through it. Where there is no mirror, `put_at` and
// `fill_row` are already empty, so such a build keeps the counters and draws nothing.
//
// Host builds come in too, and for the reason `kbd` and `v04_status` do: most of this
// file is arithmetic over bytes -- packing an event into a word, formatting a number,
// mapping a verdict to a colour -- and arithmetic is worth testing with `cargo test`
// rather than by looking at a screen. The parts that touch a screen are already behind
// their own feature gates and compile to nothing here.
#![cfg(any(
    all(target_os = "none", target_arch = "x86_64"),
    test
))]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"))]
use crate::drivers::vga::{COLOR_GREEN, COLOR_RED, COLOR_WHITE, COLOR_YELLOW};

/// One colour, named once, spelled for each surface at the point of use.
///
/// The text buffer takes a 4-bit attribute and the framebuffer takes 24-bit RGB, so a
/// single constant cannot serve both. Naming the colour instead of the encoding keeps
/// the drawing code reading the same for either.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    White,
    Green,
    Yellow,
    Red,
}

#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"))]
fn vga_attr(c: Colour) -> u8 {
    match c {
        Colour::White => COLOR_WHITE,
        Colour::Green => COLOR_GREEN,
        Colour::Yellow => COLOR_YELLOW,
        Colour::Red => COLOR_RED,
    }
}

#[cfg(feature = "gate_panel_fb")]
fn fb_rgb(c: Colour) -> u32 {
    use crate::drivers::fbcon::{FB_GREEN, FB_RED, FB_WHITE, FB_YELLOW};
    match c {
        Colour::White => FB_WHITE,
        Colour::Green => FB_GREEN,
        Colour::Yellow => FB_YELLOW,
        Colour::Red => FB_RED,
    }
}

/// Fill panel row `k` with a single character, on every surface this build has.
///
/// `k` counts the panel's own rows, `0` at its top. The two surfaces have different
/// heights -- 25 text rows against a hundred or more cell rows -- so each adds its own
/// base and the drawing code below never has to know which it is talking to.
fn fill_row(k: usize, ch: u8, colour: Colour) {
    // A build with no mirror keeps the counters and draws nowhere; the arguments are
    // then genuinely unused, and saying so is better than renaming them with underscores
    // in the signature a reader sees.
    #[cfg(not(any(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"), feature = "gate_panel_fb")))]
    let _ = (k, ch, colour);
    #[cfg(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"))]
    crate::drivers::vga::fill_row(crate::drivers::vga::LOG_HEIGHT + k, ch, vga_attr(colour));
    #[cfg(feature = "gate_panel_fb")]
    crate::drivers::fbcon::fill_row_at(crate::drivers::fbcon::log_rows() + k as u32, ch, fb_rgb(colour));
}

/// Write bytes at a cell in panel row `k`, on every surface this build has.
fn put_at(k: usize, col: usize, bytes: &[u8], colour: Colour) {
    #[cfg(not(any(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"), feature = "gate_panel_fb")))]
    let _ = (k, col, bytes, colour);
    #[cfg(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"))]
    crate::drivers::vga::put_bytes_at(crate::drivers::vga::LOG_HEIGHT + k, col, bytes, vga_attr(colour));
    #[cfg(feature = "gate_panel_fb")]
    crate::drivers::fbcon::put_bytes_at(
        crate::drivers::fbcon::log_rows() + k as u32,
        col as u32,
        bytes,
        fb_rgb(colour),
    );
}

/// Where a decision came from. The index is what gets packed into the event word.
#[derive(Clone, Copy)]
pub enum Site {
    /// The `zpl_compute` system call, issued from ring 3.
    SysCompute = 0,
    /// A write through the shared-memory gate.
    ShmWrite = 1,
    /// The scheduler picking a task on a timer tick.
    Scheduler = 2,
}

const SITE_LABELS: [&[u8]; 3] = [b"sys_compute", b"shm_write  ", b"scheduler  "];

/// Who the decision was about. Kept as a small fixed set so the panel needs no
/// buffers and no allocation: everything it prints is a `&'static [u8]` or a
/// number it formats on the stack.
#[derive(Clone, Copy)]
pub enum Who {
    Ring3User = 0,
    Shm0 = 1,
    TaskA = 2,
    TaskB = 3,
    TaskC = 4,
    TaskD = 5,
    TaskE = 6,
    Unknown = 7,
}

const WHO_LABELS: [&[u8]; 8] = [
    b"ring3 prog", b"shm_id=0  ", b"task A    ", b"task B    ",
    b"task C    ", b"task D    ", b"task E    ", b"-         ",
];

/// Map a scheduler task letter to a `Who` without a table lookup at the call site.
#[must_use]
pub fn who_for_task(id: u8) -> Who {
    match id {
        b'A' => Who::TaskA,
        b'B' => Who::TaskB,
        b'C' => Who::TaskC,
        b'D' => Who::TaskD,
        b'E' => Who::TaskE,
        _ => Who::Unknown,
    }
}

const DECISION_LABELS: [&[u8]; 3] = [b"ALLOW  ", b"DEGRADE", b"BLOCK  "];

/// The rule that produced each verdict, in the same words as the three rules in
/// `zpl_policy.rs`. A verdict with no reason next to it is a number; with the
/// reason it is an explanation.
const DECISION_REASONS: [&[u8]; 3] = [
    b"rule 1: within the demo budget",
    b"rule 3: past the per-boot quota",
    b"rule 2: over the demo limit",
];

const DECISION_COLOURS: [Colour; 3] = [Colour::Green, Colour::Yellow, Colour::Red];

/// The character that draws the rule above the panel.
///
/// `0xCD` is the double horizontal bar in code page 437, which the text buffer
/// renders. The framebuffer font covers only printable ASCII, so there it would be
/// dropped and the rule would be blank -- hence `=` when that is the surface.
#[cfg(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror"))]
const RULE_CHAR: u8 = 0xCD;
#[cfg(not(all(target_os = "none", target_arch = "x86_64", feature = "qemu_boot", feature = "vga_crit_mirror")))]
const RULE_CHAR: u8 = b'=';

static ALLOWED: AtomicU32 = AtomicU32::new(0);
static DEGRADED: AtomicU32 = AtomicU32::new(0);
static BLOCKED: AtomicU32 = AtomicU32::new(0);

/// The most recent decision **of each kind**, not the last three in time.
///
/// Chronological order looked tidy and said nothing: the scheduler decides
/// hundreds of times a second, so all three rows were the same task being allowed
/// again. One row per verdict means a glance shows what the gate did with a clean
/// request, a refused one and a throttled one — which is the question the panel
/// exists to answer.
///
/// Each is packed into one word, so no lock is needed: an interrupt landing
/// mid-update can only see a whole previous event or a whole new one.
const KINDS: usize = 3;
static EVENTS: [AtomicU64; KINDS] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

const VALID: u64 = 1 << 32;

#[inline]
const fn pack(site: u8, who: u8, score_pct: u8, decision: u8) -> u64 {
    VALID | ((site as u64) << 24) | ((who as u64) << 16) | ((score_pct as u64) << 8)
        | (decision as u64)
}

/// Record one decision and, when it is worth the redraw, put it on the screen.
pub fn note(site: Site, who: Who, score_pct: u8, decision: crate::zpl_policy::Decision) {
    let code: u8 = match decision {
        crate::zpl_policy::Decision::Allow => 0,
        crate::zpl_policy::Decision::Degrade => 1,
        crate::zpl_policy::Decision::Block => 2,
    };
    let total = match code {
        0 => ALLOWED.fetch_add(1, Ordering::Relaxed),
        1 => DEGRADED.fetch_add(1, Ordering::Relaxed),
        _ => BLOCKED.fetch_add(1, Ordering::Relaxed),
    };

    let slot = code as usize;
    let first_of_kind = EVENTS[slot].load(Ordering::Relaxed) & VALID == 0;
    EVENTS[slot].store(pack(site as u8, who as u8, score_pct, code), Ordering::Relaxed);

    // Redrawing on every tick would spend the whole timer interrupt writing cells
    // nobody is reading: the scheduler alone decides hundreds of times a second and
    // the count is the only thing that moves. The first decision of each kind is
    // drawn at once, and so is every refusal — those are the ones a visitor must not
    // miss. The rest go up every fourth decision, which is often enough that the
    // counters never look stale next to the log scrolling above them.
    if first_of_kind || code == 2 || total % 4 == 0 {
        draw();
    }
}

fn write_u32(buf: &mut [u8; 10], mut value: u32) -> usize {
    if value == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut n = 0;
    while value > 0 {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in 0..n {
        buf[i] = digits[n - 1 - i];
    }
    n
}

/// How many rows the panel owns. Seven: the heading, the counters, one row per
/// verdict, the sentence that explains what the panel is, and the summary.
pub const PANEL_ROWS: usize = 7;

/// Redraw the whole panel. Safe to call at any time; it only touches its own rows.
pub fn draw() {
    // Rule: a heading a reader can find, and a line that separates the panel from
    // the log above it. 0xCD is the double horizontal bar in code page 437, which
    // is what the text buffer renders; the framebuffer font has no such glyph, so
    // there it comes out as a row of `=`.
    fill_row(0, RULE_CHAR, Colour::White);
    put_at(0, 2, b" POLICY GATE ", Colour::White);

    let allowed = ALLOWED.load(Ordering::Relaxed);
    let degraded = DEGRADED.load(Ordering::Relaxed);
    let blocked = BLOCKED.load(Ordering::Relaxed);

    fill_row(1, b' ', Colour::White);
    let mut buf = [0u8; 10];
    let mut col = 1;

    put_at(1, col, b"ALLOW ", Colour::Green);
    col += 6;
    let n = write_u32(&mut buf, allowed);
    put_at(1, col, &buf[..n], Colour::Green);
    col += n + 4;

    put_at(1, col, b"DEGRADE ", Colour::Yellow);
    col += 8;
    let n = write_u32(&mut buf, degraded);
    put_at(1, col, &buf[..n], Colour::Yellow);
    col += n + 4;

    put_at(1, col, b"BLOCK ", Colour::Red);
    col += 6;
    let n = write_u32(&mut buf, blocked);
    put_at(1, col, &buf[..n], Colour::Red);
    col += n + 4;

    put_at(1, col, b"requests ", Colour::White);
    col += 9;
    let n = write_u32(&mut buf, allowed + degraded + blocked);
    put_at(1, col, &buf[..n], Colour::White);
    col += n;
    // Only when there is a quota to show. The demo policy has one; the engine
    // build does not, and a panel that printed one anyway would be stating a
    // limit that is not there.
    if let Some(quota) = crate::zpl_policy::demo_request_quota() {
        put_at(1, col, b" / quota ", Colour::White);
        col += 9;
        let n = write_u32(&mut buf, quota);
        put_at(1, col, &buf[..n], Colour::White);
    }

    // One row per verdict, always in the same order, so the panel does not jump
    // around while it is being read.
    for slot in 0..KINDS {
        let row = 2 + slot;
        fill_row(row, b' ', Colour::White);
        let ev = EVENTS[slot].load(Ordering::Relaxed);
        if ev & VALID == 0 {
            put_at(row, 36, DECISION_LABELS[slot], DECISION_COLOURS[slot]);
            put_at(row, 45, b"not seen yet in this boot", Colour::White);
            continue;
        }
        let site = ((ev >> 24) & 0xFF) as usize;
        let who = ((ev >> 16) & 0xFF) as usize;
        let score = ((ev >> 8) & 0xFF) as u32;
        let code = (ev & 0x3) as usize;
        let colour = DECISION_COLOURS[code.min(2)];

        put_at(row, 1, SITE_LABELS[site.min(2)], Colour::White);
        put_at(row, 14, WHO_LABELS[who.min(7)], Colour::White);
        put_at(row, 25, b"score", Colour::White);
        let n = write_u32(&mut buf, score);
        put_at(row, 31 + 3usize.saturating_sub(n), &buf[..n], Colour::White);
        put_at(row, 36, DECISION_LABELS[code.min(2)], colour);
        put_at(row, 45, DECISION_REASONS[code.min(2)], colour);
    }

    fill_row(5, b' ', Colour::White);
    // In a diagnostic build this row carries the keyboard readout instead of the
    // sentence: a photograph of a prompt that will not take input has to answer
    // "is anything arriving at all", and a row that redraws twice a second answers
    // it whenever the picture is taken. `kbd_probe` is never on in a published image.
    #[cfg(feature = "kbd_probe")]
    draw_keyboard_probe();
    #[cfg(not(feature = "kbd_probe"))]
    put_at(
        5,
        1,
        b"every state-changing request goes through the gate before it is carried out",
        Colour::White,
    );

    draw_summary();
}

/// The keyboard readout, for the diagnostic build only.
///
/// Three numbers, and each answers a different question about a prompt that will not
/// take input:
///
/// - `S` is the controller's status byte, read from the status port, which consumes
///   nothing. Bit 0 set means a byte is waiting right now.
/// - `n` is how many scancodes the kernel has actually read since boot. If this stays
///   at zero while keys are pressed, nothing is reaching the kernel and the fault is
///   below it -- the firmware's legacy emulation, not this code.
/// - `last` is the most recent scancode byte. `0x1e` is the `a` key in set 1, so a
///   sensible value here also says the controller is in the scancode set this kernel
///   decodes.
#[cfg(feature = "kbd_probe")]
fn draw_keyboard_probe() {
    let (status, seen, last) = crate::drivers::kbd::probe_snapshot();
    let mut buf = [0u8; 10];
    let mut hex = [0u8; 2];

    put_at(5, 1, b"KBD PROBE  status 0x", Colour::Yellow);
    write_hex8(&mut hex, status);
    put_at(5, 21, &hex, Colour::Yellow);

    put_at(5, 26, b"scancodes read ", Colour::Yellow);
    let n = write_u32(&mut buf, seen);
    let colour = if seen == 0 { Colour::Red } else { Colour::Green };
    put_at(5, 41, &buf[..n], colour);

    put_at(5, 48, b"last 0x", Colour::Yellow);
    write_hex8(&mut hex, last);
    put_at(5, 55, &hex, colour);

    put_at(5, 60, b"press a key and photograph this row", Colour::White);
}

/// Two lowercase hex digits for one byte.
#[cfg(feature = "kbd_probe")]
fn write_hex8(out: &mut [u8; 2], value: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out[0] = HEX[(value >> 4) as usize];
    out[1] = HEX[(value & 0x0F) as usize];
}

/// Ticks the halt loop has counted, so the summary row can show the machine is alive.
#[cfg(feature = "vga_visual_hold")]
static TICK: AtomicU32 = AtomicU32::new(0);

/// Record the halt loop's tick and redraw, so the summary row moves.
///
/// Called once per pass. The panel above it barely changes between passes, but
/// redrawing all of it costs a few hundred glyph writes twice a second, which is far
/// less than the boot log itself costs.
#[cfg(feature = "vga_visual_hold")]
pub fn note_tick(n: u32) {
    TICK.store(n, Ordering::Relaxed);
    draw();
}

/// The bottom row: version, what the self-checks did, and the last decision.
///
/// This is what used to be three rows drawn underneath the panel by the halt loop.
/// The two blocks wanted the same three rows of screen, which is why the ISO showed
/// one and never the other. Now there is one block and it is here.
fn draw_summary() {
    fill_row(6, b' ', Colour::White);
    let mut col = 1;

    put_at(6, col, b"ZPL kernel ", Colour::White);
    col += 11;
    put_at(6, col, crate::KERNEL_VERSION.as_bytes(), Colour::White);
    col += crate::KERNEL_VERSION.len() + 2;

    // The tick only means something where the halt loop counts one. On a build
    // without `vga_visual_hold` it would sit at zero for ever, which reads as a
    // stopped machine rather than as a field that does not apply.
    #[cfg(feature = "vga_visual_hold")]
    {
        let mut buf = [0u8; 10];
        put_at(6, col, b"tick ", Colour::White);
        col += 5;
        let n = write_u32(&mut buf, TICK.load(Ordering::Relaxed));
        put_at(6, col, &buf[..n], Colour::White);
        col += n + 2;
    }

    // The counters and the hash come from the same watcher the serial log feeds, so
    // what the row shows is recomputable from the log with the documented filter.
    #[cfg(feature = "v04_menu")]
    {
        let mut buf = [0u8; 10];
        let n = write_u32(&mut buf, crate::ui::v04_status::selfchecks_ok());
        put_at(6, col, &buf[..n], Colour::Green);
        col += n + 1;
        put_at(6, col, b"self-checks OK", Colour::Green);
        col += 16;

        let n = write_u32(&mut buf, crate::ui::v04_status::decisions());
        put_at(6, col, &buf[..n], Colour::White);
        col += n + 1;
        put_at(6, col, b"decisions", Colour::White);
        col += 11;

        put_at(6, col, b"hash ", Colour::White);
        col += 5;
        let mut hex = [0u8; 16];
        write_hex16(&mut hex, crate::ui::v04_status::hash());
        put_at(6, col, &hex, Colour::White);
        col += 18;

        match crate::ui::v04_status::last_decision() {
            Some((ain, action)) => {
                let colour = match action {
                    b'B' => Colour::Red,
                    b'D' => Colour::Yellow,
                    _ => Colour::Green,
                };
                put_at(6, col, b"last ain ", colour);
                col += 9;
                let n = write_u32(&mut buf, ain);
                put_at(6, col, &buf[..n], colour);
                col += n + 1;
                put_at(
                    6,
                    col,
                    match action {
                        b'B' => b"BLOCK".as_slice(),
                        b'D' => b"DEGRADE".as_slice(),
                        _ => b"ALLOW".as_slice(),
                    },
                    colour,
                );
            }
            None => put_at(6, col, b"no decision yet", Colour::White),
        }
    }
    #[cfg(not(feature = "v04_menu"))]
    let _ = col;
}

/// Sixteen lowercase hex digits, the same shape the serial-log method produces.
///
/// Only the summary row prints a hash, and only the `v04_menu` build has a watcher
/// to take one from.
#[cfg(feature = "v04_menu")]
fn write_hex16(out: &mut [u8; 16], value: u64) {
    const DIGIT: &[u8; 16] = b"0123456789abcdef";
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = (15 - i) * 4;
        *slot = DIGIT[((value >> shift) & 0xf) as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zpl_policy::Decision;

    /// The panel's event word is where a verdict and its score travel together.
    ///
    /// Worth a test because the defect this module was part of last night was exactly
    /// this: two numbers describing one decision, disagreeing. Packing is the place a
    /// score could get lost without anything failing to compile.
    #[test]
    fn an_event_word_survives_being_packed() {
        for (site, who, score, decision) in [
            (Site::SysCompute, Who::Ring3User, 0u8, 2u8),
            (Site::ShmWrite, Who::TaskA, 50, 0),
            (Site::Scheduler, Who::TaskE, 100, 1),
            (Site::Scheduler, Who::Unknown, 255, 2),
        ] {
            let w = pack(site as u8, who as u8, score, decision);
            assert_ne!(w & VALID, 0, "a packed event must read as valid");
            assert_eq!(((w >> 24) & 0xFF) as u8, site as u8);
            assert_eq!(((w >> 16) & 0xFF) as u8, who as u8);
            assert_eq!(((w >> 8) & 0xFF) as u8, score, "the score must survive");
            assert_eq!((w & 0xFF) as u8, decision);
        }
    }

    /// Zero is a real score, and an unset slot is not the same thing as a score of zero.
    #[test]
    fn a_score_of_zero_is_not_an_empty_slot() {
        let w = pack(Site::SysCompute as u8, Who::Ring3User as u8, 0, 0);
        assert_ne!(w & VALID, 0);
        assert_ne!(w, 0, "an empty slot is the literal zero word");
    }

    #[test]
    fn the_three_sites_and_the_verdicts_keep_their_indices() {
        // The index is what gets packed, so renumbering these silently moves every row
        // of the panel onto the wrong label.
        assert_eq!(Site::SysCompute as u8, 0);
        assert_eq!(Site::ShmWrite as u8, 1);
        assert_eq!(Site::Scheduler as u8, 2);
        assert_eq!(DECISION_LABELS.len(), KINDS);
        assert_eq!(DECISION_COLOURS.len(), KINDS);
    }

    #[test]
    fn a_task_letter_becomes_its_own_row_and_anything_else_is_unknown() {
        assert_eq!(who_for_task(b'A') as u8, Who::TaskA as u8);
        assert_eq!(who_for_task(b'E') as u8, Who::TaskE as u8);
        // Lower case is not a task letter; the scheduler emits upper case only.
        assert_eq!(who_for_task(b'a') as u8, Who::Unknown as u8);
        assert_eq!(who_for_task(0) as u8, Who::Unknown as u8);
    }

    #[test]
    fn numbers_are_written_without_leading_zeros_and_zero_is_one_digit() {
        let mut buf = [0u8; 10];
        for (value, want) in [
            (0u32, b"0".as_slice()),
            (7, b"7"),
            (10, b"10"),
            (168, b"168"),
            (4_294_967_295, b"4294967295"),
        ] {
            let n = write_u32(&mut buf, value);
            assert_eq!(&buf[..n], want, "write_u32({value})");
        }
    }

    // NOT covered here, and saying so rather than leaving a test that never runs:
    // `write_hex16` and the two colour maps (`vga_attr`, `fb_rgb`) live behind feature
    // gates -- `v04_menu`, `vga_crit_mirror`, `gate_panel_fb` -- whose other code calls
    // modules that exist only on bare metal. A host test of them would have to be gated
    // the same way and would then never execute in `cargo test`, which is worse than an
    // honest gap: it would look like coverage from the outside.
    //
    // What that leaves unchecked on a host: the hex formatting of the fingerprint, and
    // that each verdict gets a different colour on each surface. Both are visible on
    // every boot screenshot, which is how they have been checked so far.

    /// `note` is the only way a decision reaches the panel, so the counters it keeps are
    /// what the top row shows. A decision must land in its own counter and nowhere else.
    #[test]
    fn a_decision_counts_once_and_in_the_right_column() {
        let before = (
            ALLOWED.load(Ordering::Relaxed),
            DEGRADED.load(Ordering::Relaxed),
            BLOCKED.load(Ordering::Relaxed),
        );
        note(Site::SysCompute, Who::Ring3User, 95, Decision::Allow);
        note(Site::SysCompute, Who::Ring3User, 5, Decision::Block);
        note(Site::SysCompute, Who::Ring3User, 30, Decision::Degrade);
        note(Site::SysCompute, Who::Ring3User, 5, Decision::Block);
        let after = (
            ALLOWED.load(Ordering::Relaxed),
            DEGRADED.load(Ordering::Relaxed),
            BLOCKED.load(Ordering::Relaxed),
        );
        assert_eq!(after.0 - before.0, 1);
        assert_eq!(after.1 - before.1, 1);
        assert_eq!(after.2 - before.2, 2);
    }
}
