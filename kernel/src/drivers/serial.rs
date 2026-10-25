//! COM1 at 115200 8N1. Everything interesting the kernel says is mirrored
//! here so tests (and the demo recorder) can read it from the host.

use crate::arch::cpu::{inb, outb};
use core::fmt::{self, Write};
use spin::Mutex;

const COM1: u16 = 0x3F8;

pub struct Serial;

impl Serial {
    fn put(&mut self, b: u8) {
        while inb(COM1 + 5) & 0x20 == 0 {}
        outb(COM1, b);
    }
}

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.put(b'\r');
            }
            self.put(b);
        }
        Ok(())
    }
}

pub static SERIAL: Mutex<Serial> = Mutex::new(Serial);

pub fn init() {
    outb(COM1 + 1, 0x00); // no interrupts
    outb(COM1 + 3, 0x80); // DLAB on
    outb(COM1, 0x01); // divisor 1 -> 115200
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03); // 8N1
    outb(COM1 + 2, 0xC7); // FIFO on
    outb(COM1 + 4, 0x0B);
}

pub fn write_fmt(args: fmt::Arguments) {
    // Interrupts are off whenever the kernel runs, so a plain lock is fine;
    // force_unlock is only used by the panic handler.
    let _ = SERIAL.lock().write_fmt(args);
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => { $crate::drivers::serial::write_fmt(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! serial_println {
    () => { $crate::serial_print!("\n") };
    ($($arg:tt)*) => { $crate::serial_print!("{}\n", format_args!($($arg)*)) };
}
