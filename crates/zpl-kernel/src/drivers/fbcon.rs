//! Framebuffer text console for the Limine / higher-half boot path (v0.4).
//!
//! `vga.rs` mirrors critical markers into the legacy 80x25 text buffer at `0xB8000`.
//! That buffer is invisible once the bootloader hands us a **graphical** framebuffer,
//! which is exactly what Limine v12.2.0 does: on physical hardware (20 Sep 2026) the
//! kernel booted and kept talking on COM1 while the screen stayed black. This module
//! draws the same markers as 8x8 glyphs into that framebuffer instead.
//!
//! **Font:** the glyph table below was drawn by hand for this repository. It is original
//! work carrying the repository's own licence — no third-party font data is embedded, so
//! there is no external font licence to track.
//!
//! **Only 32 bpp is supported.** At any other depth the console refuses to draw a single
//! pixel and says so on COM1; a half-drawn screen is worse than a black one.
//!
//! Compiled only when **`fb_crit_mirror`** is on. `zpl-kernel-bin` never enables it, so
//! the QEMU `-kernel` build is unaffected.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const GLYPH_FIRST: u8 = 0x20;
const GLYPH_LAST: u8 = 0x7E;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;
/// Glyph height plus 2 px of leading. Without the gap, descenders in one row touch the
/// caps of the next and the screen is hard to read in a photo of a monitor.
const CELL_H: usize = 10;

/// 8x8 glyphs for ASCII `0x20..=0x7E`; bit 7 of each row is the leftmost pixel.
#[rustfmt::skip]
static FONT8X8: [[u8; GLYPH_H]; (GLYPH_LAST - GLYPH_FIRST + 1) as usize] = [
    // 0x20   
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    // 0x21  !
    [0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x00],
    // 0x22  "
    [0x36, 0x36, 0x36, 0x00, 0x00, 0x00, 0x00, 0x00],
    // 0x23  #
    [0x36, 0x36, 0x7F, 0x36, 0x7F, 0x36, 0x36, 0x00],
    // 0x24  $
    [0x18, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x18, 0x00],
    // 0x25  %
    [0x62, 0x66, 0x0C, 0x18, 0x30, 0x66, 0x46, 0x00],
    // 0x26  &
    [0x38, 0x6C, 0x38, 0x76, 0x6E, 0x66, 0x3D, 0x00],
    // 0x27  '
    [0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00],
    // 0x28  (
    [0x0C, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00],
    // 0x29  )
    [0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00],
    // 0x2A  *
    [0x00, 0x2A, 0x1C, 0x7F, 0x1C, 0x2A, 0x00, 0x00],
    // 0x2B  +
    [0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00],
    // 0x2C  ,
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30],
    // 0x2D  -
    [0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00],
    // 0x2E  .
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00],
    // 0x2F  /
    [0x06, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x60, 0x00],
    // 0x30  0
    [0x3C, 0x66, 0x6E, 0x7D, 0x76, 0x66, 0x3C, 0x00],
    // 0x31  1
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
    // 0x32  2
    [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x30, 0x7F, 0x00],
    // 0x33  3
    [0x7F, 0x0C, 0x18, 0x0E, 0x06, 0x66, 0x3C, 0x00],
    // 0x34  4
    [0x0E, 0x1E, 0x36, 0x66, 0x7F, 0x06, 0x0F, 0x00],
    // 0x35  5
    [0x7F, 0x60, 0x7C, 0x03, 0x03, 0x66, 0x3C, 0x00],
    // 0x36  6
    [0x1C, 0x30, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0x00],
    // 0x37  7
    [0x7F, 0x66, 0x0C, 0x18, 0x18, 0x18, 0x18, 0x00],
    // 0x38  8
    [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0x00],
    // 0x39  9
    [0x3C, 0x66, 0x66, 0x3E, 0x03, 0x0C, 0x38, 0x00],
    // 0x3A  :
    [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00],
    // 0x3B  ;
    [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x30],
    // 0x3C  <
    [0x0E, 0x18, 0x30, 0x60, 0x30, 0x18, 0x0E, 0x00],
    // 0x3D  =
    [0x00, 0x00, 0x7E, 0x00, 0x7E, 0x00, 0x00, 0x00],
    // 0x3E  >
    [0x70, 0x18, 0x0C, 0x06, 0x0C, 0x18, 0x70, 0x00],
    // 0x3F  ?
    [0x3C, 0x66, 0x0C, 0x18, 0x18, 0x00, 0x18, 0x00],
    // 0x40  @
    [0x3C, 0x66, 0x6E, 0x6E, 0x60, 0x30, 0x1E, 0x00],
    // 0x41  A
    [0x18, 0x3C, 0x66, 0x66, 0x7F, 0x66, 0x66, 0x00],
    // 0x42  B
    [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0x00],
    // 0x43  C
    [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00],
    // 0x44  D
    [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0x00],
    // 0x45  E
    [0x7F, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x7F, 0x00],
    // 0x46  F
    [0x7F, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0x00],
    // 0x47  G
    [0x3C, 0x66, 0x60, 0x6E, 0x66, 0x66, 0x3C, 0x00],
    // 0x48  H
    [0x66, 0x66, 0x66, 0x7F, 0x66, 0x66, 0x66, 0x00],
    // 0x49  I
    [0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
    // 0x4A  J
    [0x0F, 0x06, 0x06, 0x06, 0x66, 0x66, 0x3C, 0x00],
    // 0x4B  K
    [0x66, 0x6C, 0x78, 0x70, 0x78, 0x6C, 0x66, 0x00],
    // 0x4C  L
    [0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7F, 0x00],
    // 0x4D  M
    [0x63, 0x77, 0x7F, 0x6B, 0x63, 0x63, 0x63, 0x00],
    // 0x4E  N
    [0x63, 0x73, 0x7B, 0x6F, 0x67, 0x63, 0x63, 0x00],
    // 0x4F  O
    [0x3C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
    // 0x50  P
    [0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x00],
    // 0x51  Q
    [0x3C, 0x66, 0x66, 0x66, 0x6E, 0x6C, 0x3B, 0x00],
    // 0x52  R
    [0x7C, 0x66, 0x66, 0x7C, 0x78, 0x6C, 0x66, 0x00],
    // 0x53  S
    [0x3E, 0x60, 0x60, 0x3C, 0x03, 0x06, 0x7C, 0x00],
    // 0x54  T
    [0x7F, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
    // 0x55  U
    [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
    // 0x56  V
    [0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00],
    // 0x57  W
    [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00],
    // 0x58  X
    [0x66, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x66, 0x00],
    // 0x59  Y
    [0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x18, 0x00],
    // 0x5A  Z
    [0x7F, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x7F, 0x00],
    // 0x5B  [
    [0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00],
    // 0x5C  \\
    [0x60, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x06, 0x00],
    // 0x5D  ]
    [0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00],
    // 0x5E  ^
    [0x18, 0x3C, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00],
    // 0x5F  _
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7F],
    // 0x60  `
    [0x30, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    // 0x61  a
    [0x00, 0x00, 0x3C, 0x03, 0x3E, 0x66, 0x3E, 0x00],
    // 0x62  b
    [0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x7C, 0x00],
    // 0x63  c
    [0x00, 0x00, 0x3E, 0x60, 0x60, 0x60, 0x3E, 0x00],
    // 0x64  d
    [0x03, 0x03, 0x3E, 0x66, 0x66, 0x66, 0x3E, 0x00],
    // 0x65  e
    [0x00, 0x00, 0x3C, 0x66, 0x7F, 0x60, 0x3E, 0x00],
    // 0x66  f
    [0x0E, 0x18, 0x3E, 0x18, 0x18, 0x18, 0x18, 0x00],
    // 0x67  g
    [0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x03, 0x3C],
    // 0x68  h
    [0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x00],
    // 0x69  i
    [0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x3C, 0x00],
    // 0x6A  j
    [0x0C, 0x00, 0x1C, 0x0C, 0x0C, 0x0C, 0x38, 0x00],
    // 0x6B  k
    [0x60, 0x60, 0x66, 0x6C, 0x78, 0x6C, 0x66, 0x00],
    // 0x6C  l
    [0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00],
    // 0x6D  m
    [0x00, 0x00, 0x6C, 0x7F, 0x6B, 0x63, 0x63, 0x00],
    // 0x6E  n
    [0x00, 0x00, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x00],
    // 0x6F  o
    [0x00, 0x00, 0x3C, 0x66, 0x66, 0x66, 0x3C, 0x00],
    // 0x70  p
    [0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60],
    // 0x71  q
    [0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x03, 0x03],
    // 0x72  r
    [0x00, 0x00, 0x6E, 0x73, 0x60, 0x60, 0x60, 0x00],
    // 0x73  s
    [0x00, 0x00, 0x3E, 0x60, 0x3C, 0x03, 0x7C, 0x00],
    // 0x74  t
    [0x18, 0x18, 0x3E, 0x18, 0x18, 0x1B, 0x0E, 0x00],
    // 0x75  u
    [0x00, 0x00, 0x66, 0x66, 0x66, 0x66, 0x3E, 0x00],
    // 0x76  v
    [0x00, 0x00, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00],
    // 0x77  w
    [0x00, 0x00, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00],
    // 0x78  x
    [0x00, 0x00, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x00],
    // 0x79  y
    [0x00, 0x00, 0x66, 0x66, 0x66, 0x3E, 0x03, 0x3C],
    // 0x7A  z
    [0x00, 0x00, 0x7F, 0x0C, 0x18, 0x30, 0x7F, 0x00],
    // 0x7B  {
    [0x0E, 0x18, 0x18, 0x30, 0x18, 0x18, 0x0E, 0x00],
    // 0x7C  |
    [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
    // 0x7D  }
    [0x70, 0x0C, 0x0C, 0x06, 0x0C, 0x0C, 0x70, 0x00],
    // 0x7E  ~
    [0x3B, 0x6E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
];

/// Packed `0x00RRGGBB`; converted to the framebuffer's own channel layout on write.
pub const FB_WHITE: u32 = 0x00CC_CCCC;
pub const FB_GREEN: u32 = 0x0000_CC00;
pub const FB_YELLOW: u32 = 0x00CC_CC00;
pub const FB_RED: u32 = 0x00CC_0000;

static FB_ADDR: AtomicU64 = AtomicU64::new(0);
static FB_WIDTH: AtomicU64 = AtomicU64::new(0);
static FB_HEIGHT: AtomicU64 = AtomicU64::new(0);
static FB_PITCH: AtomicU64 = AtomicU64::new(0);
static FB_BPP: AtomicU32 = AtomicU32::new(0);
static FB_R_SHIFT: AtomicU32 = AtomicU32::new(16);
static FB_G_SHIFT: AtomicU32 = AtomicU32::new(8);
static FB_B_SHIFT: AtomicU32 = AtomicU32::new(0);
static FB_READY: AtomicBool = AtomicBool::new(false);
static FB_CLEARED: AtomicBool = AtomicBool::new(false);
/// The status row is blanked lazily, right before the next glyph lands on it. Blanking it
/// eagerly inside `newline` erased the line that had just been written.
static PENDING_ROW_CLEAR: AtomicBool = AtomicBool::new(false);
/// Cursor parked by [`save_cursor`] while the status line is written.
static SAVED_ROW: AtomicU32 = AtomicU32::new(0);
static SAVED_COL: AtomicU32 = AtomicU32::new(0);

static COL: AtomicU32 = AtomicU32::new(0);
static ROW: AtomicU32 = AtomicU32::new(0);
static CUR_COLOR: AtomicU32 = AtomicU32::new(FB_WHITE);

/// Record the framebuffer Limine handed us. Called once from `limine-bridge` **after**
/// `paging::set_hhdm_offset`, before entering the kernel.
///
/// `addr` is stored exactly as the bootloader reported it. Limine is documented to return
/// a higher-half pointer, but instead of trusting that, [`fb_base`] checks at draw time
/// whether the value looks physical and adds the HHDM offset if so. The raw value is
/// printed on COM1 so the log shows which case actually hit.
///
/// An unsupported depth is recorded but leaves `FB_READY` false, so nothing is ever drawn.
#[allow(clippy::too_many_arguments)]
pub fn publish_framebuffer(
    addr: u64,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
    red_shift: u8,
    green_shift: u8,
    blue_shift: u8,
) {
    FB_ADDR.store(addr, Ordering::Release);
    FB_WIDTH.store(width, Ordering::Release);
    FB_HEIGHT.store(height, Ordering::Release);
    FB_PITCH.store(pitch, Ordering::Release);
    FB_BPP.store(u32::from(bpp), Ordering::Release);
    FB_R_SHIFT.store(u32::from(red_shift), Ordering::Release);
    FB_G_SHIFT.store(u32::from(green_shift), Ordering::Release);
    FB_B_SHIFT.store(u32::from(blue_shift), Ordering::Release);
    let usable = bpp == 32 && addr != 0 && width >= GLYPH_W as u64 && height >= CELL_H as u64;
    FB_READY.store(usable, Ordering::Release);
}

/// True once a usable 32-bpp framebuffer has been published.
#[must_use]
pub fn is_fb_ready() -> bool {
    FB_READY.load(Ordering::Acquire)
}

/// Raw address as published by the bootloader (0 when none) — for the COM1 trace line.
#[must_use]
pub fn raw_addr() -> u64 {
    FB_ADDR.load(Ordering::Acquire)
}

#[must_use]
pub fn bpp() -> u32 {
    FB_BPP.load(Ordering::Acquire)
}

#[must_use]
pub fn dimensions() -> (u64, u64, u64) {
    (
        FB_WIDTH.load(Ordering::Acquire),
        FB_HEIGHT.load(Ordering::Acquire),
        FB_PITCH.load(Ordering::Acquire),
    )
}

/// Rows at the bottom the policy-gate panel owns, when it is drawn here.
///
/// Mirrors `vga::PANEL_ROWS`, and for the same reason: text that flowed into those rows
/// would be painted over by the next panel redraw, so the text region has to stop above
/// them. Zero when the panel is not compiled, which leaves the console exactly as it was.
#[cfg(feature = "gate_panel_fb")]
pub const PANEL_ROWS: u32 = 7;
#[cfg(not(feature = "gate_panel_fb"))]
pub const PANEL_ROWS: u32 = 0;

/// Rows ordinary output may use. Everything below belongs to the panel.
#[must_use]
pub fn log_rows() -> u32 {
    let (_, rows) = grid();
    rows.saturating_sub(PANEL_ROWS)
}

/// Write `bytes` starting at a cell, clipped at the right edge. Does not move the cursor.
///
/// The panel addresses cells directly instead of writing a stream, so a redraw cannot
/// disturb where ordinary output had got to.
pub fn put_bytes_at(row: u32, col: u32, bytes: &[u8], rgb: u32) {
    if !is_fb_ready() {
        return;
    }
    let (cols, rows) = grid();
    if row >= rows {
        return;
    }
    let colour = pack(rgb);
    for (i, &b) in bytes.iter().enumerate() {
        let x = col + i as u32;
        if x >= cols {
            return;
        }
        draw_glyph_over(b, x, row, colour, true);
    }
}

/// Fill a whole row with one character. Does not move the cursor.
pub fn fill_row_at(row: u32, ch: u8, rgb: u32) {
    if !is_fb_ready() {
        return;
    }
    let (cols, rows) = grid();
    if row >= rows {
        return;
    }
    clear_row(row);
    if ch == b' ' {
        return;
    }
    let colour = pack(rgb);
    for x in 0..cols {
        draw_glyph(ch, x, row, colour);
    }
}

/// Columns / rows of 8x8 cells that fit on screen.
#[must_use]
pub fn grid() -> (u32, u32) {
    let (w, h, _) = dimensions();
    ((w / GLYPH_W as u64) as u32, (h / CELL_H as u64) as u32)
}

/// Linear address to draw into.
///
/// Limine returns a higher-half pointer, but a published value below the HHDM offset can
/// only be physical, so we translate it. Checking here — instead of assuming one
/// convention — keeps the same binary correct either way.
#[inline]
fn fb_base() -> usize {
    let addr = FB_ADDR.load(Ordering::Acquire);
    match crate::mm::phys_hhdm::current_hhdm_offset() {
        Some(off) if addr < off => off.wrapping_add(addr) as usize,
        _ => addr as usize,
    }
}

#[inline]
fn pack(rgb: u32) -> u32 {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    (r << FB_R_SHIFT.load(Ordering::Relaxed))
        | (g << FB_G_SHIFT.load(Ordering::Relaxed))
        | (b << FB_B_SHIFT.load(Ordering::Relaxed))
}

#[inline]
fn put_pixel(x: u64, y: u64, colour: u32) {
    let (w, h, pitch) = dimensions();
    if x >= w || y >= h {
        return;
    }
    let offset = y.wrapping_mul(pitch).wrapping_add(x.wrapping_mul(4)) as usize;
    unsafe {
        let p = (fb_base() + offset) as *mut u32;
        core::ptr::write_volatile(p, colour);
    }
}

/// Paint the whole framebuffer black and put the cursor home.
pub fn clear() {
    if !is_fb_ready() {
        return;
    }
    let (w, h, _) = dimensions();
    for y in 0..h {
        for x in 0..w {
            put_pixel(x, y, 0);
        }
    }
    COL.store(0, Ordering::Relaxed);
    ROW.store(0, Ordering::Relaxed);
    FB_CLEARED.store(true, Ordering::Release);
}

/// Set the colour used by subsequent writes (one of the `FB_*` constants).
pub fn set_color(rgb: u32) {
    CUR_COLOR.store(rgb, Ordering::Relaxed);
}

/// Pick a colour for a whole marker line, mirroring `vga::attr_for_marker_bytes`.
#[must_use]
pub fn color_for_marker_bytes(bytes: &[u8]) -> u32 {
    if contains(bytes, b"action=BLOCK") {
        return FB_RED;
    }
    if contains(bytes, b"action=DEGRADE") {
        return FB_YELLOW;
    }
    if contains(bytes, b"OK") || contains(bytes, b"action=ALLOW") {
        return FB_GREEN;
    }
    FB_WHITE
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Blank a single glyph row.
/// Paint one cell black.
fn clear_cell(cell_x: u32, cell_y: u32) {
    let ox = u64::from(cell_x) * GLYPH_W as u64;
    let oy = u64::from(cell_y) * CELL_H as u64;
    for y in oy..oy + CELL_H as u64 {
        for x in ox..ox + GLYPH_W as u64 {
            put_pixel(x, y, 0);
        }
    }
}

fn clear_row(row: u32) {
    let (w, _, _) = dimensions();
    let oy = u64::from(row) * CELL_H as u64;
    for y in oy..oy + CELL_H as u64 {
        for x in 0..w {
            put_pixel(x, y, 0);
        }
    }
}

fn newline() {
    let (_, rows) = grid();
    COL.store(0, Ordering::Relaxed);
    if rows == 0 {
        return;
    }
    let next = ROW.load(Ordering::Relaxed) + 1;
    // `rows` is the whole screen; the text region stops above the panel.
    let bottom = rows.saturating_sub(PANEL_ROWS).max(1);
    if next >= bottom {
        // Bottom reached: the last row becomes a status line that overwrites itself,
        // and everything above it stays on screen. Wiping the whole framebuffer here
        // would erase the self-check output on every `[ZPL-ALIVE]` tick — which is
        // exactly what made the screen look dead during v0.4 bring-up.
        ROW.store(bottom - 1, Ordering::Relaxed);
        PENDING_ROW_CLEAR.store(true, Ordering::Release);
    } else {
        ROW.store(next, Ordering::Relaxed);
    }
}

fn draw_glyph(ch: u8, cell_x: u32, cell_y: u32, colour: u32) {
    draw_glyph_over(ch, cell_x, cell_y, colour, false);
}

/// Draw a glyph, optionally wiping the cell first.
///
/// The console writes forward into rows it has already cleared, so it does not need the
/// wipe and pays for eighty pixel writes per character if it takes one. The panel writes
/// into cells that already hold something -- its own previous contents, or the rule it
/// is labelling -- and without the wipe a space leaves what was underneath: the heading
/// came out as `POLICY=GATE`, the `=` of the rule showing through the gap.
fn draw_glyph_over(ch: u8, cell_x: u32, cell_y: u32, colour: u32, wipe: bool) {
    if !(GLYPH_FIRST..=GLYPH_LAST).contains(&ch) {
        return;
    }
    if wipe {
        clear_cell(cell_x, cell_y);
    }
    let glyph = &FONT8X8[(ch - GLYPH_FIRST) as usize];
    let ox = u64::from(cell_x) * GLYPH_W as u64;
    let oy = u64::from(cell_y) * CELL_H as u64;
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..GLYPH_W {
            if bits & (0x80 >> col) != 0 {
                put_pixel(ox + col as u64, oy + row as u64, colour);
            }
        }
    }
}

pub fn write_byte(b: u8) {
    if !is_fb_ready() {
        return;
    }
    if !FB_CLEARED.load(Ordering::Acquire) {
        clear();
    }
    let colour = pack(CUR_COLOR.load(Ordering::Relaxed));
    match b {
        b'\n' => newline(),
        b'\r' => COL.store(0, Ordering::Relaxed),
        b'\t' => {
            for _ in 0..4 {
                write_byte(b' ');
            }
        }
        _ => {
            let (cols, _) = grid();
            if cols == 0 {
                return;
            }
            if COL.load(Ordering::Relaxed) >= cols {
                newline();
            }
            let x = COL.load(Ordering::Relaxed);
            let y = ROW.load(Ordering::Relaxed);
            if PENDING_ROW_CLEAR.swap(false, Ordering::AcqRel) {
                clear_row(y);
            }
            draw_glyph(b, x, y, colour);
            COL.store(x + 1, Ordering::Relaxed);
        }
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}

pub fn write_bytes(bytes: &[u8]) {
    for &b in bytes {
        write_byte(b);
    }
}

/// Remember where normal output had got to, before writing the bottom status line.
///
/// Without this pair the `[ZPL-ALIVE]` ticker leaves the cursor on the bottom row, so the
/// next ordinary line lands there too and the ticker overwrites it half a second later —
/// which is what hid the menu results during v0.4 bring-up.
pub fn save_cursor() {
    SAVED_ROW.store(ROW.load(Ordering::Relaxed), Ordering::Relaxed);
    SAVED_COL.store(COL.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Put the cursor back where [`save_cursor`] left it.
pub fn restore_cursor() {
    ROW.store(SAVED_ROW.load(Ordering::Relaxed), Ordering::Relaxed);
    COL.store(SAVED_COL.load(Ordering::Relaxed), Ordering::Relaxed);
    PENDING_ROW_CLEAR.store(false, Ordering::Release);
}

/// Move the cursor to the start of a row counted from the bottom (`0` = bottom row).
///
/// Used for the v0.4 summary block, which owns the last few rows while ordinary output
/// keeps flowing above it. Counted from the bottom of the **text region**, so with the
/// panel compiled in it lands above the panel rather than under it.
pub fn jump_cursor_row_from_bottom(k: u32) {
    if !is_fb_ready() {
        return;
    }
    let rows = log_rows();
    if rows == 0 || k >= rows {
        return;
    }
    if !FB_CLEARED.load(Ordering::Acquire) {
        clear();
    }
    let row = rows - 1 - k;
    ROW.store(row, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
    clear_row(row);
    PENDING_ROW_CLEAR.store(false, Ordering::Release);
}

/// Move the cursor to the start of the bottom row (mirrors `vga::jump_cursor_last_row_start`).
pub fn jump_cursor_last_row_start() {
    if !is_fb_ready() {
        return;
    }
    let rows = log_rows();
    if rows == 0 {
        return;
    }
    if !FB_CLEARED.load(Ordering::Acquire) {
        clear();
    }
    ROW.store(rows - 1, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
    PENDING_ROW_CLEAR.store(true, Ordering::Release);
}
