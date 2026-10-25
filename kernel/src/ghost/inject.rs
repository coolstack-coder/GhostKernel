//! Fault injection. These aren't simulated crashes: we corrupt the saved
//! CPU state of a process, so the next time it runs the *hardware* raises
//! a real exception in ring 3, exactly like a wild pointer or a smashed
//! stack would.

use crate::proc::Process;

pub fn describe(kind: u8) -> &'static str {
    match kind {
        0 => "rip <- 0x0",
        1 => "rip <- non-canonical",
        2 => "rsp <- 0x1000",
        _ => "rip ^= 1<<31 (bit flip)",
    }
}

pub fn apply(p: &mut Process, kind: u8) {
    let c = &mut p.ctx;
    match kind {
        0 => c.rip = 0,
        1 => c.rip = 0xDEAD_BEEF_0000_0000,
        2 => c.rsp = 0x1000,
        // a single high bit flip lands rip in the kernel's half of the map
        _ => c.rip ^= 1 << 31,
    }
}
