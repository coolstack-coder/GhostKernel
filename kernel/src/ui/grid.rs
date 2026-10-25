use super::font::{self, CELL_H, CELL_W, Target};
use super::theme;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
}

impl Cell {
    pub const BLANK: Cell = Cell {
        ch: ' ',
        fg: theme::TEXT,
        bg: theme::BG,
        bold: false,
    };
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    shown: Vec<Cell>,
    target: Target,
    force: bool,
}

/// A styled pen for writing into the grid.
#[derive(Clone, Copy)]
pub struct Style {
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
}

impl Style {
    pub const fn new(fg: u32, bg: u32) -> Self {
        Style {
            fg,
            bg,
            bold: false,
        }
    }
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}

// The framebuffer pointer is only ever touched under the GRID lock.
unsafe impl Send for Grid {}

impl Grid {
    pub fn new(fb: u64, width: usize, height: usize) -> Self {
        let cols = width / CELL_W;
        let rows = height / CELL_H;
        Grid {
            cols,
            rows,
            cells: vec![Cell::BLANK; cols * rows],
            shown: vec![
                Cell {
                    ch: '\0',
                    ..Cell::BLANK
                };
                cols * rows
            ],
            target: Target {
                base: fb as *mut u32,
                stride: width,
            },
            force: true,
        }
    }

    pub fn clear(&mut self, bg: u32) {
        self.cells.fill(Cell { bg, ..Cell::BLANK });
    }

    pub fn set(&mut self, x: usize, y: usize, c: Cell) {
        if x < self.cols && y < self.rows {
            self.cells[y * self.cols + x] = c;
        }
    }

    pub fn put(&mut self, x: usize, y: usize, ch: char, s: Style) {
        self.set(
            x,
            y,
            Cell {
                ch,
                fg: s.fg,
                bg: s.bg,
                bold: s.bold,
            },
        );
    }

    /// Write a string clipped to `max` columns; returns the column after it.
    pub fn text(&mut self, x: usize, y: usize, s: &str, st: Style, max: usize) -> usize {
        let mut cx = x;
        for ch in s.chars() {
            if cx >= x + max || cx >= self.cols {
                break;
            }
            self.put(cx, y, ch, st);
            cx += 1;
        }
        cx
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, ch: char, st: Style) {
        for yy in y..(y + h).min(self.rows) {
            for xx in x..(x + w).min(self.cols) {
                self.put(xx, yy, ch, st);
            }
        }
    }

    /// Rounded panel with a title chip in the top border.
    pub fn panel(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        title: &str,
        accent: u32,
        border: u32,
    ) {
        let b = Style::new(border, theme::BG);
        self.fill(
            x + 1,
            y + 1,
            w - 2,
            h - 2,
            ' ',
            Style::new(theme::TEXT, theme::PANEL),
        );
        for xx in x + 1..x + w - 1 {
            self.put(xx, y, '─', b);
            self.put(xx, y + h - 1, '─', b);
        }
        for yy in y + 1..y + h - 1 {
            self.put(x, yy, '│', Style::new(border, theme::PANEL));
            self.put(x + w - 1, yy, '│', Style::new(border, theme::PANEL));
        }
        // the corners' outer half sits on the page background
        self.put(x, y, '╭', b);
        self.put(x + w - 1, y, '╮', b);
        self.put(x, y + h - 1, '╰', b);
        self.put(x + w - 1, y + h - 1, '╯', b);
        let cx = self.text(x + 2, y, " ", b, 1);
        self.put(cx, y, '◆', Style::new(accent, theme::BG));
        self.text(cx + 1, y, " ", b, 1);
        let end = self.text(
            cx + 2,
            y,
            title,
            Style::new(theme::WHITE, theme::BG).bold(),
            w - 8,
        );
        self.text(end, y, " ", b, 1);
    }

    /// Large text over a block of cells. Call after `flush`; the cells
    /// underneath must stay unchanged while it's visible.
    pub fn big_text(&mut self, col: usize, row: usize, s: &str, fg: u32, bg: u32) {
        let mut px = col * CELL_W;
        for ch in s.chars() {
            font::draw_big(&self.target, px, row * CELL_H, ch, fg, bg);
            px += font::BIG_W;
        }
    }

    /// Push changed cells to the framebuffer.
    pub fn flush(&mut self) -> usize {
        let mut painted = 0;
        for y in 0..self.rows {
            for x in 0..self.cols {
                let i = y * self.cols + x;
                let c = self.cells[i];
                if self.force || c != self.shown[i] {
                    font::draw_cell(
                        &self.target,
                        x * CELL_W,
                        y * CELL_H,
                        c.ch,
                        c.fg,
                        c.bg,
                        c.bold,
                    );
                    self.shown[i] = c;
                    painted += 1;
                }
            }
        }
        self.force = false;
        painted
    }
}
