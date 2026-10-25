//! Bochs Graphics Adapter (QEMU's `-vga std`). Four port writes and we get a
//! linear 32-bit framebuffer - no BIOS calls, no mode tables.

use crate::arch::cpu::{inw, outw};

const INDEX: u16 = 0x01CE;
const DATA: u16 = 0x01CF;

const REG_ID: u16 = 0;
const REG_XRES: u16 = 1;
const REG_YRES: u16 = 2;
const REG_BPP: u16 = 3;
const REG_ENABLE: u16 = 4;

fn write(reg: u16, val: u16) {
    outw(INDEX, reg);
    outw(DATA, val);
}

fn read(reg: u16) -> u16 {
    outw(INDEX, reg);
    inw(DATA)
}

pub struct Mode {
    pub base: u64,
    pub width: usize,
    pub height: usize,
}

pub fn init(width: u16, height: u16) -> Option<Mode> {
    let id = read(REG_ID);
    if !(0xB0C0..=0xB0CF).contains(&id) {
        return None;
    }
    let dev = super::pci::find(0x1234, 0x1111)?;
    write(REG_ENABLE, 0);
    write(REG_XRES, width);
    write(REG_YRES, height);
    write(REG_BPP, 32);
    write(REG_ENABLE, 0x01 | 0x40); // enabled + linear framebuffer
    Some(Mode {
        base: dev.bar0 as u64,
        width: width as usize,
        height: height as usize,
    })
}
