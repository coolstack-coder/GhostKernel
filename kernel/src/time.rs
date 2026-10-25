//! Wall-clock-ish timing via the TSC, calibrated against the PIT at boot.

use crate::arch::{cpu, pit};
use core::sync::atomic::{AtomicU64, Ordering};

static TSC_PER_MS: AtomicU64 = AtomicU64::new(1_000_000);

/// Needs interrupts on and the PIT ticking.
pub fn calibrate() {
    let start = pit::ticks();
    while pit::ticks() == start {
        core::hint::spin_loop();
    }
    let t0 = cpu::rdtsc();
    let from = pit::ticks();
    while pit::ticks() < from + 10 {
        core::hint::spin_loop();
    }
    let per_ms = (cpu::rdtsc() - t0) / (10 * 1000 / pit::HZ);
    TSC_PER_MS.store(per_ms.max(1), Ordering::Relaxed);
}

pub fn tsc() -> u64 {
    cpu::rdtsc()
}

pub fn tsc_to_us(d: u64) -> u64 {
    d * 1000 / TSC_PER_MS.load(Ordering::Relaxed)
}

pub fn mhz() -> u64 {
    TSC_PER_MS.load(Ordering::Relaxed) / 1000
}
