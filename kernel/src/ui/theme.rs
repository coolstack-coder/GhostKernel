//! "Midnight ghost" palette. RGB, 0xRRGGBB.

pub const BG: u32 = 0x0A0D14;
pub const PANEL: u32 = 0x0F131C;
pub const HEADER: u32 = 0x121826;
pub const BORDER: u32 = 0x262E40;
pub const TEXT: u32 = 0xC9D1D9;
pub const DIM: u32 = 0x5C6680;
pub const FAINT: u32 = 0x343C50;
pub const TRACK: u32 = 0x1A2030;
pub const GHOST: u32 = 0x5EEAD4; // teal - the brand colour
pub const PURPLE: u32 = 0xA78BFA;
pub const GREEN: u32 = 0x4ADE80;
pub const YELLOW: u32 = 0xFACC15;
pub const ORANGE: u32 = 0xFB923C;
pub const RED: u32 = 0xF87171;
pub const RED_DEEP: u32 = 0x7F1D1D;
pub const GREEN_DEEP: u32 = 0x14532D;
pub const BLUE: u32 = 0x60A5FA;
pub const PINK: u32 = 0xF472B6;
pub const WHITE: u32 = 0xF5F7FA;

/// Per-process accent so interleaved console output stays readable.
pub fn proc_color(pid: u32) -> u32 {
    const P: [u32; 7] = [BLUE, PINK, YELLOW, GREEN, PURPLE, ORANGE, GHOST];
    P[pid as usize % P.len()]
}

pub fn blend(bg: u32, fg: u32, a: u8) -> u32 {
    let a = a as u32;
    let ch = |s: u32| {
        let b = (bg >> s) & 0xFF;
        let f = (fg >> s) & 0xFF;
        ((f * a + b * (255 - a)) / 255) << s
    };
    ch(16) | ch(8) | ch(0)
}
