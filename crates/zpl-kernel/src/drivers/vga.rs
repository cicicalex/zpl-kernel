//! VGA text-mode buffer (80×25, 16 CGA colors) for Limine / higher-half boot.
//!
//! Physical base `0xB8000`; virtual address = [`crate::mm::phys_hhdm::current_hhdm_offset`]
//! plus that physical base when HHDM is active, else identity-mapped `0xB8000`.
//!
//! Compiled whenever **`vga_crit_mirror`** is on, which the public build turns
//! on for itself. It used to be kept out of `qemu_boot` builds to keep
//! `zpl-kernel-bin` byte-identical; the cost was that a plain `-kernel` boot
//! could put nothing on a screen at all.

use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};

/// CGA foreground on black background (attribute high nibble = 0).
pub const COLOR_GREEN: u8 = 2;
pub const COLOR_YELLOW: u8 = 14;
pub const COLOR_RED: u8 = 4;
pub const COLOR_WHITE: u8 = 15;

const VGA_PHYS_BASE: u64 = 0xB8000;
pub(crate) const WIDTH: usize = 80;
const HEIGHT: usize = 25;

/// Rows the policy panel owns at the bottom of the screen.
///
/// The scrolling log stops above them, so the panel stays put while boot output
/// goes past. Only on the `-kernel` path: the Limine build draws its own summary
/// on the framebuffer and does not use this buffer for it.
#[cfg(all(feature = "qemu_boot", feature = "vga_crit_mirror"))]
pub(crate) const PANEL_ROWS: usize = 7;
#[cfg(not(all(feature = "qemu_boot", feature = "vga_crit_mirror")))]
pub(crate) const PANEL_ROWS: usize = 0;

/// Rows the scrolling log may use. Everything below belongs to the panel.
pub(crate) const LOG_HEIGHT: usize = HEIGHT - PANEL_ROWS;

static ROW: AtomicU16 = AtomicU16::new(0);
static COL: AtomicU16 = AtomicU16::new(0);
static CURRENT_ATTR: AtomicU16 = AtomicU16::new(COLOR_WHITE as u16);
static INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Linear address of the 80×25 text cells (each `u16`: char + attribute).
///
/// Returns **`current_hhdm_offset() + 0xB8000`** when Limine provided an HHDM
/// offset (Limine does not cover legacy 0xA0000–0xC0000 in that window by
/// default). After `paging::init` on the non–`qemu_boot` path, that VA is
/// backed by an explicit identity-style leaf PTE to physical `0xB8000`, so
/// `vga_crit_mirror` writes do not fault. Without HHDM (`None`), this is bare
/// **`0xB8000`** (Multiboot / identity boot).
#[inline]
fn vga_buffer_base() -> usize {
    match crate::mm::phys_hhdm::current_hhdm_offset() {
        Some(off) => off.wrapping_add(VGA_PHYS_BASE) as usize,
        None => VGA_PHYS_BASE as usize,
    }
}

#[inline]
const fn cell(ch: u8, attr: u8) -> u16 {
    (ch as u16) | ((attr as u16) << 8)
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Pick VGA foreground attribute for a whole marker line (COM1 mirror).
#[must_use]
pub fn attr_for_marker_bytes(bytes: &[u8]) -> u8 {
    if contains_subslice(bytes, b"action=BLOCK") {
        return COLOR_RED;
    }
    if contains_subslice(bytes, b"action=DEGRADE") {
        return COLOR_YELLOW;
    }
    if contains_subslice(bytes, b"action=ALLOW") {
        return COLOR_GREEN;
    }
    // The scheduler writes a decision line in seven pieces, and the verdict arrives
    // as a fragment of its own -- just `DEGRADE`, with the `action=` in the piece
    // before it. Matched exactly, not as a substring, so a line that merely mentions
    // one of these words in prose keeps its normal colour.
    match bytes {
        b"BLOCK" => return COLOR_RED,
        b"DEGRADE" => return COLOR_YELLOW,
        b"ALLOW" => return COLOR_GREEN,
        _ => {}
    }
    // A failed self-check in the same white as a passing one is easy to miss on a
    // screen that is mostly white text. Every `FAIL` marker in the kernel spells it
    // in capitals, and no passing marker contains the word.
    if contains_subslice(bytes, b"FAIL") {
        return COLOR_RED;
    }
    COLOR_WHITE
}

#[inline]
pub fn set_color(fg: u8) {
    CURRENT_ATTR.store((fg & 0x0F) as u16, Ordering::Relaxed);
}

/// Write one cell of the text buffer.
///
/// `write_volatile`, not `write`: this is a device's memory, and a plain write
/// lets the compiler merge a run of them into vector stores. Those are legal
/// against RAM and fine under TCG, but a hypervisor that emulates MMIO decodes
/// only a subset of instructions -- under WHPX the first such store ends the
/// machine, with no message and no serial output. Measured: the same writes
/// issued one at a time from assembly survive there, the compiled Rust ones
/// did not.
unsafe fn put_cell(row: usize, col: usize, value: u16) {
    let idx = row * WIDTH + col;
    let ptr = (vga_buffer_base() as *mut u16).add(idx);
    core::ptr::write_volatile(ptr, value);
}

unsafe fn scroll_one_line(attr_fill: u8) {
    // #region agent log
    #[cfg(feature = "agent_debug_boot")]
    crate::agent_debug_com1::ndjson_line(crate::agent_debug_com1::DBG_VGA_SCROLL_ENTER);
    // #endregion agent log
    let base = vga_buffer_base() as *mut u16;
    // Only the log region scrolls. `LOG_HEIGHT` is the whole screen unless the
    // panel is compiled in, so the Limine path behaves exactly as before.
    core::ptr::copy(base.add(WIDTH), base, WIDTH * (LOG_HEIGHT - 1));
    let fill = cell(b' ', attr_fill);
    for c in 0..WIDTH {
        core::ptr::write_volatile(base.add(WIDTH * (LOG_HEIGHT - 1) + c), fill);
    }
}

fn ensure_init() {
    if INIT_DONE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    // No full-screen clear on first use: without an explicit PTE for 0xB8000 in the
    // HHDM window, MMIO to the legacy text buffer can #PF (CR2≈HHDM+0xB8000); with an
    // empty IDT that can cascade to #DF / triple fault — often mistaken for a “stall”
    // under QEMU + SeaBIOS VBE + ISO (see PHYSICAL_TEST_GUIDE). Cursor starts
    // at top-left; prior firmware text remains until cells are overwritten incrementally.
    ROW.store(0, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
}

fn newline() {
    let mut row = ROW.load(Ordering::Relaxed) as usize;
    let attr = CURRENT_ATTR.load(Ordering::Relaxed) as u8;

    // Blank the rest of the row before leaving it. Without this a short line
    // leaves the tail of whatever longer line was there before, so the screen
    // fills with fragments like `]2144` hanging off the right of otherwise
    // correct output. Only on the `qemu_boot` path, where the whole buffer is
    // identity-mapped and already cleared once at entry; the Limine path avoids
    // extra writes for the reason in `ensure_init`.
    #[cfg(feature = "qemu_boot")]
    {
        let col = COL.load(Ordering::Relaxed) as usize;
        unsafe {
            let base = vga_buffer_base() as *mut u16;
            let blank = cell(b' ', attr);
            for c in col..WIDTH {
                core::ptr::write_volatile(base.add(row * WIDTH + c), blank);
            }
        }
    }

    if row < LOG_HEIGHT - 1 {
        row += 1;
        ROW.store(row as u16, Ordering::Relaxed);
        COL.store(0, Ordering::Relaxed);
    } else {
        unsafe {
            scroll_one_line(attr);
        }
        COL.store(0, Ordering::Relaxed);
    }
}

/// Clear the text buffer and reset the cursor (uses current attribute for spaces).
pub fn clear() {
    ensure_init();
    let attr = CURRENT_ATTR.load(Ordering::Relaxed) as u8;
    let fill = cell(b' ', attr);
    unsafe {
        let base = vga_buffer_base() as *mut u16;
        for i in 0..(WIDTH * HEIGHT) {
            core::ptr::write_volatile(base.add(i), fill);
        }
    }
    ROW.store(0, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
}

/// Move cursor to the bottom-left cell so short status lines overwrite the same row.
pub fn jump_cursor_last_row_start() {
    ensure_init();
    ROW.store((LOG_HEIGHT - 1) as u16, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
}

pub fn write_byte(b: u8) {
    ensure_init();
    let attr = CURRENT_ATTR.load(Ordering::Relaxed) as u8;
    match b {
        b'\n' => newline(),
        b'\r' => {
            COL.store(0, Ordering::Relaxed);
        }
        b'\t' => {
            for _ in 0..4 {
                write_byte(b' ');
            }
        }
        _ => {
            let mut col = COL.load(Ordering::Relaxed) as usize;
            let mut row = ROW.load(Ordering::Relaxed) as usize;
            if col >= WIDTH {
                newline();
                col = COL.load(Ordering::Relaxed) as usize;
                row = ROW.load(Ordering::Relaxed) as usize;
            }
            unsafe {
                put_cell(row, col, cell(b, attr));
            }
            col += 1;
            COL.store(col as u16, Ordering::Relaxed);
        }
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}

/// Write `bytes` at an absolute cell position, without moving the log cursor.
///
/// The panel owns fixed rows and redraws them in place, so it must not go
/// through `write_byte` — that would scroll the log and fight with it. Writes
/// past the right edge are dropped rather than wrapping.
// Only the panel uses these, and the panel is not compiled on the Limine path.
#[cfg(all(feature = "qemu_boot", feature = "vga_crit_mirror"))]
pub(crate) fn put_bytes_at(row: usize, col: usize, bytes: &[u8], attr: u8) {
    ensure_init();
    if row >= HEIGHT {
        return;
    }
    unsafe {
        let base = vga_buffer_base() as *mut u16;
        for (i, b) in bytes.iter().enumerate() {
            let c = col + i;
            if c >= WIDTH {
                break;
            }
            core::ptr::write_volatile(base.add(row * WIDTH + c), cell(*b, attr));
        }
    }
}

/// Fill a whole row with one character, e.g. to blank it before redrawing.
// Only the panel uses these, and the panel is not compiled on the Limine path.
#[cfg(all(feature = "qemu_boot", feature = "vga_crit_mirror"))]
pub(crate) fn fill_row(row: usize, ch: u8, attr: u8) {
    ensure_init();
    if row >= HEIGHT {
        return;
    }
    unsafe {
        let base = vga_buffer_base() as *mut u16;
        let value = cell(ch, attr);
        for c in 0..WIDTH {
            core::ptr::write_volatile(base.add(row * WIDTH + c), value);
        }
    }
}
