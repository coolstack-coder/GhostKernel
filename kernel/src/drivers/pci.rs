//! Just enough PCI config-space access (mechanism #1) to find the display
//! adapter and its framebuffer BAR.

use crate::arch::cpu::{inl, outl};

fn read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let addr = 0x8000_0000u32
        | (bus as u32) << 16
        | (dev as u32) << 11
        | (func as u32) << 8
        | (off as u32 & 0xFC);
    outl(0xCF8, addr);
    inl(0xCFC)
}

pub struct Device {
    pub bar0: u32,
}

pub fn find(vendor: u16, device: u16) -> Option<Device> {
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            let id = read32(bus, dev, 0, 0);
            if id == 0xFFFF_FFFF {
                continue;
            }
            if id as u16 == vendor && (id >> 16) as u16 == device {
                return Some(Device {
                    bar0: read32(bus, dev, 0, 0x10) & !0xF,
                });
            }
        }
    }
    None
}
