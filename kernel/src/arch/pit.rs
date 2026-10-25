use super::cpu::outb;
use core::sync::atomic::{AtomicU64, Ordering};

pub const HZ: u64 = 100;
static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    let divisor = (1_193_182 / HZ) as u16;
    outb(0x43, 0x36); // channel 0, lo/hi, square wave
    outb(0x40, divisor as u8);
    outb(0x40, (divisor >> 8) as u8);
}

pub fn tick() -> u64 {
    TICKS.fetch_add(1, Ordering::Relaxed) + 1
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    ticks() * 1000 / HZ
}
