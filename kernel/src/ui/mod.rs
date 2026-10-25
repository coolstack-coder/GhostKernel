//! The dashboard: a 1280x800 framebuffer split into a grid of 7x16 cells.
//!
//! Each frame we compose the whole grid from kernel state, diff it against
//! what is already on screen and only repaint the cells that changed. That
//! keeps redraws cheap enough to run from the timer interrupt, even under
//! QEMU's software CPU.

pub mod dashboard;
pub mod font;
pub mod grid;
pub mod state;
pub mod theme;

pub use state::{Ev, console_write, event};

pub const SCREEN_W: u16 = 1280;
pub const SCREEN_H: u16 = 800;
