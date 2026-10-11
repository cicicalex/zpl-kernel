//! A large-print text screen for the diagnostic, on the bootloader's framebuffer.
//!
//! Text is kept in a character grid first and drawn second. Lines written before the
//! framebuffer is known -- the first stages run before it -- therefore still appear the
//! moment it is, and a redraw only touches the cells that changed, which keeps the
//! live counters cheap on a machine whose framebuffer is uncached.

use crate::drivers::fbcon::glyph_rows;

use super::decode::pick_scale;

/// Logical layout width. The diagnostic never writes past it, whatever the screen is.
pub const COLS: usize = 80;
/// The tallest grid kept. A 1x screen can have more rows than this; they stay black.
pub const ROWS_MAX: usize = 80;
/// Rows assumed before the framebuffer is known.
const ROWS_DEFAULT: usize = 36;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    White = 0,
    Green = 1,
    Yellow = 2,
    Red = 3,
    Cyan = 4,
    Grey = 5,
}

impl Color {
    fn rgb(self) -> u32 {
        match self {
            Color::White => 0x00E0_E0E0,
            Color::Green => 0x0040_E040,
            Color::Yellow => 0x00F0_D020,
            Color::Red => 0x00FF_4040,
            Color::Cyan => 0x0040_D0F0,
            Color::Grey => 0x0090_9090,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: u8,
    color: Color,
}

const BLANK: Cell = Cell { ch: b' ', color: Color::White };

struct Fb {
    base: u64,
    width: u64,
    height: u64,
    pitch: u64,
    bytes_pp: u64,
    shifts: [u32; 3],
    scale: u64,
    rows: usize,
}

/// Everything the screen owns. One instance, touched only by the single CPU the
/// diagnostic runs on, with interrupts off -- the exception path included, which runs
/// on that same CPU and never returns to what it interrupted.
struct Screen {
    want: [[Cell; COLS]; ROWS_MAX],
    drawn: [[Cell; COLS]; ROWS_MAX],
    fb: Option<Fb>,
}

static mut SCREEN: Screen = Screen {
    want: [[BLANK; COLS]; ROWS_MAX],
    drawn: [[BLANK; COLS]; ROWS_MAX],
    fb: None,
};

fn screen() -> &'static mut Screen {
    // SAFETY: see `Screen`. There is no second CPU and no interrupt handler that comes
    // back, so no two references are ever live at once.
    unsafe { &mut *core::ptr::addr_of_mut!(SCREEN) }
}

/// Why a framebuffer could not be used, for the stage line.
pub enum AttachError {
    Depth(u16),
    TooSmall,
}

/// Take over the framebuffer. `base` must be an address the CPU can write right now.
///
/// 32 and 24 bits per pixel are drawn; anything else is refused, with the depth named.
#[allow(clippy::too_many_arguments)]
pub fn attach(
    base: u64,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
    r_shift: u8,
    g_shift: u8,
    b_shift: u8,
) -> Result<(u32, usize, usize), AttachError> {
    let bytes_pp = match bpp {
        32 => 4,
        24 => 3,
        other => return Err(AttachError::Depth(other)),
    };
    let scale = u64::from(pick_scale(width, height));
    let rows = ((height / (10 * scale)) as usize).min(ROWS_MAX);
    let cols = (width / (8 * scale)) as usize;
    if rows < 10 || cols < 40 {
        return Err(AttachError::TooSmall);
    }
    let s = screen();
    s.fb = Some(Fb {
        base,
        width,
        height,
        pitch,
        bytes_pp,
        shifts: [u32::from(r_shift), u32::from(g_shift), u32::from(b_shift)],
        scale,
        rows,
    });
    fill_black();
    s.drawn = [[BLANK; COLS]; ROWS_MAX];
    flush();
    Ok((scale as u32, cols.min(COLS), rows))
}

/// Rows of text the screen has (a default before there is a screen).
#[must_use]
pub fn rows() -> usize {
    screen().fb.as_ref().map_or(ROWS_DEFAULT, |fb| fb.rows)
}

/// Write `bytes` into row `row` from column `col`, clipped at the layout's edge.
pub fn put(row: usize, col: usize, bytes: &[u8], color: Color) {
    let s = screen();
    let Some(line) = s.want.get_mut(row) else { return };
    for (i, &b) in bytes.iter().enumerate() {
        let Some(cell) = line.get_mut(col + i) else { return };
        *cell = Cell { ch: b, color };
    }
}

/// Blank row `row` from column `col` to the end.
pub fn clear_from(row: usize, col: usize) {
    let s = screen();
    let Some(line) = s.want.get_mut(row) else { return };
    for cell in line.iter_mut().skip(col) {
        *cell = BLANK;
    }
}

/// Draw every cell that differs from what is on the screen.
pub fn flush() {
    let s = screen();
    let Some(fb) = s.fb.as_ref() else { return };
    for row in 0..fb.rows {
        for col in 0..COLS {
            let cell = s.want[row][col];
            if s.drawn[row][col] != cell {
                draw_cell(fb, row, col, cell);
                s.drawn[row][col] = cell;
            }
        }
    }
}

fn pack(fb: &Fb, rgb: u32) -> u32 {
    let [r, g, b] = fb.shifts;
    (((rgb >> 16) & 0xFF) << r) | (((rgb >> 8) & 0xFF) << g) | ((rgb & 0xFF) << b)
}

#[inline]
fn put_pixel(fb: &Fb, x: u64, y: u64, value: u32) {
    if x >= fb.width || y >= fb.height {
        return;
    }
    let at = fb.base + y * fb.pitch + x * fb.bytes_pp;
    // SAFETY: inside the framebuffer the bootloader described, which `attach` was told
    // is writable; the bounds were checked just above.
    unsafe {
        if fb.bytes_pp == 4 {
            core::ptr::write_volatile(at as *mut u32, value);
        } else {
            let p = at as *mut u8;
            core::ptr::write_volatile(p, value as u8);
            core::ptr::write_volatile(p.add(1), (value >> 8) as u8);
            core::ptr::write_volatile(p.add(2), (value >> 16) as u8);
        }
    }
}

fn fill_black() {
    let s = screen();
    let Some(fb) = s.fb.as_ref() else { return };
    for y in 0..fb.height {
        for x in 0..fb.width {
            put_pixel(fb, x, y, 0);
        }
    }
}

fn draw_cell(fb: &Fb, row: usize, col: usize, cell: Cell) {
    let fg = pack(fb, cell.color.rgb());
    let glyph = glyph_rows(cell.ch);
    let x0 = col as u64 * 8 * fb.scale;
    let y0 = row as u64 * 10 * fb.scale;
    for gy in 0..10u64 {
        // Rows 8 and 9 of the cell are the gap between lines.
        let bits = glyph.and_then(|g| g.get(gy as usize)).copied().unwrap_or(0);
        for gx in 0..8u64 {
            let value = if bits & (0x80 >> gx) != 0 { fg } else { 0 };
            for sy in 0..fb.scale {
                for sx in 0..fb.scale {
                    put_pixel(fb, x0 + gx * fb.scale + sx, y0 + gy * fb.scale + sy, value);
                }
            }
        }
    }
}
