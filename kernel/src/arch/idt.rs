use super::{gdt, trap};
use core::arch::asm;
use core::mem::size_of;

#[repr(C)]
#[derive(Clone, Copy)]
struct Gate {
    offset_lo: u16,
    selector: u16,
    ist: u8,
    attrs: u8,
    offset_mid: u16,
    offset_hi: u32,
    _zero: u32,
}

impl Gate {
    const MISSING: Gate = Gate {
        offset_lo: 0,
        selector: 0,
        ist: 0,
        attrs: 0,
        offset_mid: 0,
        offset_hi: 0,
        _zero: 0,
    };

    fn new(handler: u64, dpl: u8, ist: u8) -> Self {
        Gate {
            offset_lo: handler as u16,
            selector: gdt::KERNEL_CS,
            ist,
            attrs: 0x8E | (dpl << 5), // present, 64-bit interrupt gate (IF cleared on entry)
            offset_mid: (handler >> 16) as u16,
            offset_hi: (handler >> 32) as u32,
            _zero: 0,
        }
    }
}

static mut IDT: [Gate; 256] = [Gate::MISSING; 256];

#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}

pub fn init() {
    unsafe {
        for v in 0..48 {
            // the double fault gets its own known-good stack (IST1)
            let ist = if v == 8 { 1 } else { 0 };
            IDT[v] = Gate::new(trap::stub_address(v), 0, ist);
        }
        // DPL 3 so user code is allowed to `int 0x80`
        IDT[0x80] = Gate::new(trap::stub_address(0x80), 3, 0);

        let ptr = IdtPtr {
            limit: (size_of::<[Gate; 256]>() - 1) as u16,
            base: (&raw const IDT) as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr, options(nostack));
    }
}
