//! The venerable 8259 pair. We remap it past the CPU exceptions (32..47)
//! and only unmask what we actually use: the timer and the keyboard.

use super::cpu::{inb, io_wait, outb};

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

pub fn init() {
    outb(PIC1_CMD, 0x11);
    io_wait();
    outb(PIC2_CMD, 0x11);
    io_wait();
    outb(PIC1_DATA, 32);
    io_wait();
    outb(PIC2_DATA, 40);
    io_wait();
    outb(PIC1_DATA, 4);
    io_wait();
    outb(PIC2_DATA, 2);
    io_wait();
    outb(PIC1_DATA, 0x01);
    io_wait();
    outb(PIC2_DATA, 0x01);
    io_wait();

    // unmask IRQ0 (timer), IRQ1 (keyboard), IRQ2 (cascade)
    outb(PIC1_DATA, !0b0000_0111);
    outb(PIC2_DATA, 0xFF);
}

pub fn eoi(irq: u8) {
    if irq >= 8 {
        outb(PIC2_CMD, 0x20);
    }
    outb(PIC1_CMD, 0x20);
}

/// IRQ7/IRQ15 can fire spuriously; only acknowledge them if the ISR says
/// they were real.
pub fn spurious(vector: u8) {
    let irq = vector - 32;
    let cmd = if irq >= 8 { PIC2_CMD } else { PIC1_CMD };
    outb(cmd, 0x0B);
    let isr = inb(cmd);
    if isr & (1 << (irq % 8)) != 0 {
        eoi(irq);
    } else if irq >= 8 {
        outb(PIC1_CMD, 0x20);
    }
}
