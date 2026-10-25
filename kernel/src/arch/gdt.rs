//! Segments still matter in long mode for exactly two things: which ring
//! we're in, and the TSS that tells the CPU which stack to use when an
//! interrupt drags it from ring 3 back into ring 0.

use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_DS: u16 = 0x10;
pub const USER_DS: u16 = 0x18 | 3;
pub const USER_CS: u16 = 0x20 | 3;
const TSS_SEL: u16 = 0x28;

#[repr(C, packed)]
struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

#[repr(C, packed)]
struct DescriptorPtr {
    limit: u16,
    base: u64,
}

const TRAP_STACK_SIZE: usize = 64 * 1024;
const DF_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct Stack<const N: usize>([u8; N]);

// Every trap out of ring 3 lands on this one stack. That's enough because
// the kernel never sleeps in the middle of a syscall: it either finishes or
// switches to another process's saved frame before returning.
static mut TRAP_STACK: Stack<TRAP_STACK_SIZE> = Stack([0; TRAP_STACK_SIZE]);
static mut DF_STACK: Stack<DF_STACK_SIZE> = Stack([0; DF_STACK_SIZE]);

static mut TSS: Tss = Tss {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    iomap_base: size_of::<Tss>() as u16,
};

static mut GDT: [u64; 7] = [
    0,
    0x00AF9A000000FFFF, // kernel code
    0x00CF92000000FFFF, // kernel data
    0x00CFF2000000FFFF, // user data (DPL 3)
    0x00AFFA000000FFFF, // user code (DPL 3)
    0,                  // TSS low  - filled at runtime
    0,                  // TSS high
];

pub fn init() {
    unsafe {
        let trap_top = (&raw const TRAP_STACK) as u64 + TRAP_STACK_SIZE as u64;
        let df_top = (&raw const DF_STACK) as u64 + DF_STACK_SIZE as u64;
        TSS.rsp[0] = trap_top;
        TSS.ist[0] = df_top;

        let base = (&raw const TSS) as u64;
        let limit = (size_of::<Tss>() - 1) as u64;
        GDT[5] = (limit & 0xFFFF)
            | ((base & 0xFF_FFFF) << 16)
            | (0x89 << 40) // present, 64-bit available TSS
            | (((limit >> 16) & 0xF) << 48)
            | (((base >> 24) & 0xFF) << 56);
        GDT[6] = base >> 32;

        let ptr = DescriptorPtr {
            limit: (size_of::<[u64; 7]>() - 1) as u16,
            base: (&raw const GDT) as u64,
        };
        asm!(
            "lgdt [{ptr}]",
            // reload CS with a far return, then the data segments
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ds, {ds:x}",
            "mov es, {ds:x}",
            "mov ss, {ds:x}",
            "ltr {tss:x}",
            ptr = in(reg) &ptr,
            cs = in(reg) KERNEL_CS as u64,
            ds = in(reg) KERNEL_DS as u64,
            tss = in(reg) TSS_SEL as u64,
            tmp = lateout(reg) _,
        );
    }
}
