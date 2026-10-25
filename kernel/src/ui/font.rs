//! Glyph rendering. Text comes from the Noto Sans Mono bitmap font; box
//! drawing, blocks, sparkline bars and a few icons are drawn procedurally
//! because the font doesn't ship them and they need to tile perfectly.

use super::theme::blend;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster};

pub const CELL_W: usize = 7;
pub const CELL_H: usize = 16;
pub const BIG_W: usize = 14;
pub const BIG_H: usize = 32;

pub struct Target {
    pub base: *mut u32,
    pub stride: usize, // in pixels
}

/// Paint one cell at pixel position (px, py).
pub fn draw_cell(t: &Target, px: usize, py: usize, ch: char, fg: u32, bg: u32, bold: bool) {
    let mut cell = [[0u8; CELL_W]; CELL_H];
    if !procedural(ch, &mut cell) {
        let weight = if bold {
            FontWeight::Bold
        } else {
            FontWeight::Regular
        };
        let r = get_raster(ch, weight, RasterHeight::Size16)
            .or_else(|| get_raster('?', weight, RasterHeight::Size16))
            .unwrap();
        for (y, row) in r.raster().iter().enumerate().take(CELL_H) {
            for (x, &a) in row.iter().enumerate().take(CELL_W) {
                cell[y][x] = a;
            }
        }
    }
    for (y, row) in cell.iter().enumerate() {
        let line = unsafe { t.base.add((py + y) * t.stride + px) };
        for (x, &a) in row.iter().enumerate() {
            let c = match a {
                0 => bg,
                255 => fg,
                a => blend(bg, fg, a),
            };
            unsafe { line.add(x).write_volatile(c) };
        }
    }
}

/// 32px bold text for banners, drawn straight onto the framebuffer.
pub fn draw_big(t: &Target, px: usize, py: usize, ch: char, fg: u32, bg: u32) {
    let Some(r) = get_raster(ch, FontWeight::Bold, RasterHeight::Size32) else {
        return;
    };
    for (y, row) in r.raster().iter().enumerate().take(BIG_H) {
        let line = unsafe { t.base.add((py + y) * t.stride + px) };
        for (x, &a) in row.iter().enumerate().take(BIG_W) {
            unsafe { line.add(x).write_volatile(blend(bg, fg, a)) };
        }
    }
}

fn hline(c: &mut [[u8; CELL_W]; CELL_H], y: usize, x0: usize, x1: usize) {
    for x in x0..x1 {
        c[y][x] = 255;
    }
}

fn vline(c: &mut [[u8; CELL_W]; CELL_H], x: usize, y0: usize, y1: usize) {
    for row in c.iter_mut().take(y1).skip(y0) {
        row[x] = 255;
    }
}

fn fill(c: &mut [[u8; CELL_W]; CELL_H], x0: usize, y0: usize, x1: usize, y1: usize, a: u8) {
    for row in c.iter_mut().take(y1).skip(y0) {
        for v in row.iter_mut().take(x1).skip(x0) {
            *v = a;
        }
    }
}

fn disc(c: &mut [[u8; CELL_W]; CELL_H], r2: i32, ring: bool) {
    // centre at (3, 8) in a 7x16 cell
    for (y, row) in c.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            let dx = x as i32 - 3;
            let dy = y as i32 - 8;
            let d = dx * dx + dy * dy;
            if d <= r2 && (!ring || d >= r2 - 4) {
                *v = 255;
            }
        }
    }
}

/// Returns true if `ch` was handled here.
fn procedural(ch: char, c: &mut [[u8; CELL_W]; CELL_H]) -> bool {
    const MX: usize = 3; // centre column
    const MY: usize = 8; // centre row
    match ch {
        '─' => hline(c, MY, 0, CELL_W),
        '│' => vline(c, MX, 0, CELL_H),
        '┼' => {
            hline(c, MY, 0, CELL_W);
            vline(c, MX, 0, CELL_H);
        }
        '├' => {
            vline(c, MX, 0, CELL_H);
            hline(c, MY, MX, CELL_W);
        }
        '┤' => {
            vline(c, MX, 0, CELL_H);
            hline(c, MY, 0, MX + 1);
        }
        '┬' => {
            hline(c, MY, 0, CELL_W);
            vline(c, MX, MY, CELL_H);
        }
        '┴' => {
            hline(c, MY, 0, CELL_W);
            vline(c, MX, 0, MY + 1);
        }
        // rounded corners: two lines that stop short, plus a soft pixel in the bend
        '╭' => {
            hline(c, MY, MX + 2, CELL_W);
            vline(c, MX, MY + 2, CELL_H);
            c[MY][MX + 1] = 160;
            c[MY + 1][MX] = 160;
            c[MY + 1][MX + 1] = 90;
        }
        '╮' => {
            hline(c, MY, 0, MX - 1);
            vline(c, MX, MY + 2, CELL_H);
            c[MY][MX - 1] = 160;
            c[MY + 1][MX] = 160;
            c[MY + 1][MX - 1] = 90;
        }
        '╰' => {
            hline(c, MY, MX + 2, CELL_W);
            vline(c, MX, 0, MY - 1);
            c[MY][MX + 1] = 160;
            c[MY - 1][MX] = 160;
            c[MY - 1][MX + 1] = 90;
        }
        '╯' => {
            hline(c, MY, 0, MX - 1);
            vline(c, MX, 0, MY - 1);
            c[MY][MX - 1] = 160;
            c[MY - 1][MX] = 160;
            c[MY - 1][MX - 1] = 90;
        }
        '█' => fill(c, 0, 0, CELL_W, CELL_H, 255),
        '▀' => fill(c, 0, 0, CELL_W, CELL_H / 2, 255),
        '▄' => fill(c, 0, CELL_H / 2, CELL_W, CELL_H, 255),
        '░' => fill(c, 0, 0, CELL_W, CELL_H, 60),
        '▒' => fill(c, 0, 0, CELL_W, CELL_H, 120),
        '▓' => fill(c, 0, 0, CELL_W, CELL_H, 190),
        // lower eighth blocks: sparklines
        '▁'..='▇' => {
            let n = ch as usize - '▁' as usize + 1; // 1..=7
            let h = (CELL_H * n + 4) / 8;
            fill(c, 0, CELL_H - h, CELL_W, CELL_H, 255);
        }
        // left eighth blocks: smooth progress bars (▉ = 7/8 ... ▏ = 1/8)
        '▉'..='▏' => {
            let n = 8 - (ch as usize - '▉' as usize + 1); // 7..=1
            let w = (CELL_W * n + 4) / 8;
            fill(c, 0, 0, w.max(1), CELL_H, 255);
        }
        '●' => disc(c, 10, false),
        '○' => disc(c, 10, true),
        '•' => disc(c, 3, false),
        '◆' => {
            for (y, row) in c.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    let d = (x as i32 - 3).abs() + (y as i32 - 8).abs();
                    if d <= 3 {
                        *v = 255;
                    }
                }
            }
        }
        '▶' => {
            for y in 4..13usize {
                let w = 4 - (y as i32 - 8).unsigned_abs() as usize;
                for x in 1..(1 + w.min(5)) {
                    c[y][x] = 255;
                }
            }
        }
        '✖' => {
            // two-pixel strokes so it reads as red, not grey, at this size
            for i in 0..7 {
                let y = 4 + i;
                c[y][i] = 255;
                c[y][6 - i] = 255;
                c[y + 1][i] = 200;
                c[y + 1][6 - i] = 200;
            }
        }
        '✔' => {
            let pts = [(0, 9), (1, 10), (2, 11), (3, 10), (4, 9), (5, 8), (6, 7)];
            for (x, y) in pts {
                c[y][x] = 255;
                c[y + 1][x] = 255;
            }
        }
        '↻' => {
            disc(c, 10, true);
            disc(c, 6, true);
            // arrow head at the top right
            c[4][5] = 255;
            c[4][6] = 255;
            c[5][6] = 255;
            c[3][4] = 255;
            c[5][4] = 0;
            c[6][5] = 0;
        }
        'Δ' => {
            for y in 4..13usize {
                let half = (y - 4) * 3 / 8;
                c[y][3 - half] = 255;
                c[y][3 + half] = 255;
            }
            hline(c, 12, 0, CELL_W);
        }
        '⚡' => {
            let pts = [
                (4, 3),
                (3, 4),
                (3, 5),
                (2, 6),
                (2, 7),
                (1, 8),
                (2, 8),
                (3, 8),
                (4, 8),
                (4, 9),
                (3, 10),
                (3, 11),
                (2, 12),
                (5, 8),
            ];
            for (x, y) in pts {
                c[y][x] = 255;
            }
        }
        _ => return false,
    }
    true
}
