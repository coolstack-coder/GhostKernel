//! Thin, honest wrappers around the instructions Rust can't express.

use core::arch::asm;

#[inline]
pub fn outb(port: u16, val: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags))
    }
}

#[inline]
pub fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe {
        asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags))
    }
    v
}

#[inline]
pub fn outw(port: u16, val: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags))
    }
}

#[inline]
pub fn inw(port: u16) -> u16 {
    let v: u16;
    unsafe {
        asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags))
    }
    v
}

#[inline]
pub fn outl(port: u16, val: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags))
    }
}

#[inline]
pub fn inl(port: u16) -> u32 {
    let v: u32;
    unsafe {
        asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags))
    }
    v
}

/// A write to an unused port; gives slow devices (the PIC) time to settle.
#[inline]
pub fn io_wait() {
    outb(0x80, 0);
}

#[inline]
pub fn read_cr2() -> u64 {
    let v: u64;
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

#[inline]
pub fn read_cr3() -> u64 {
    let v: u64;
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}

/// # Safety
/// `pml4` must point at a valid top-level table that maps the running kernel.
#[inline]
pub unsafe fn write_cr3(pml4: u64) {
    unsafe { asm!("mov cr3, {}", in(reg) pml4, options(nostack, preserves_flags)) }
}

#[inline]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) }
    ((hi as u64) << 32) | lo as u64
}

#[inline]
pub fn hlt() {
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
}

#[inline]
pub fn cli() {
    unsafe { asm!("cli", options(nomem, nostack)) }
}

#[inline]
pub fn sti() {
    unsafe { asm!("sti", options(nomem, nostack)) }
}

pub fn halt_forever() -> ! {
    loop {
        cli();
        hlt();
    }
}

/// Exit QEMU through the isa-debug-exit device (only wired up in test runs).
/// QEMU reports `(code << 1) | 1` to the host.
#[cfg_attr(not(feature = "selftest"), allow(dead_code))]
pub fn qemu_exit(code: u32) -> ! {
    outl(0xf4, code);
    halt_forever()
}
