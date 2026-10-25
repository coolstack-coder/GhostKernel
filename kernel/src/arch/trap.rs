//! One door into the kernel.
//!
//! Exceptions, hardware IRQs and `int 0x80` syscalls all go through the same
//! assembly stub and produce the same `TrapFrame`. That uniformity is the
//! whole trick behind GhostKernel: a context switch is "copy a frame out,
//! copy another frame in", and a checkpoint is "copy the frame + memory".

use core::arch::global_asm;

/// Exactly what `trap_common` leaves on the stack, lowest address first.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    // pushed by the CPU
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 22 * 8);

impl TrapFrame {
    #[allow(clippy::wrong_self_convention)]
    pub fn from_user(&self) -> bool {
        self.cs & 3 == 3
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, core::mem::size_of::<Self>())
        }
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        assert!(b.len() >= core::mem::size_of::<Self>());
        unsafe { core::ptr::read_unaligned(b.as_ptr() as *const Self) }
    }
}

global_asm!(
    r#"
.macro TRAP_NOERR n
trap_stub_\n:
    push 0
    push \n
    jmp trap_common
.endm

.macro TRAP_ERR n
trap_stub_\n:
    push \n
    jmp trap_common
.endm

.section .text
.irp n, 0,1,2,3,4,5,6,7,9,15,16,18,19,20,22,23,24,25,26,27,28,31
    TRAP_NOERR \n
.endr
.irp n, 8,10,11,12,13,14,17,21,29,30
    TRAP_ERR \n
.endr
.irp n, 32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,128
    TRAP_NOERR \n
.endr

trap_common:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    cld
    call trap_dispatch
.global trap_restore
trap_restore:
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq

/* Jump into a frame without having trapped first (first process, idle). */
.global trap_enter
trap_enter:
    mov rsp, rdi
    jmp trap_restore

.section .rodata
.align 8
.global trap_stub_table
trap_stub_table:
.irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47
    .quad trap_stub_\n
.endr
.global trap_stub_syscall
trap_stub_syscall:
    .quad trap_stub_128
"#
);

unsafe extern "C" {
    static trap_stub_table: [u64; 48];
    static trap_stub_syscall: u64;
    fn trap_enter(frame: *const TrapFrame) -> !;
}

pub fn stub_address(vector: usize) -> u64 {
    unsafe {
        if vector == 0x80 {
            trap_stub_syscall
        } else {
            trap_stub_table[vector]
        }
    }
}

/// Load `frame` into the CPU and `iretq` into it. Never returns.
///
/// # Safety
/// The frame must describe a valid context, and for ring 3 the right CR3
/// must already be loaded.
pub unsafe fn enter(frame: &TrapFrame) -> ! {
    // Copy onto the current stack top-ish area: trap_enter makes rsp point
    // at the frame and pops from it, so it must outlive the jump.
    unsafe { trap_enter(frame as *const TrapFrame) }
}

pub const EXCEPTION_NAMES: [&str; 32] = [
    "divide error",
    "debug",
    "NMI",
    "breakpoint",
    "overflow",
    "bound range",
    "invalid opcode",
    "device not available",
    "double fault",
    "coprocessor overrun",
    "invalid TSS",
    "segment not present",
    "stack fault",
    "general protection",
    "page fault",
    "reserved",
    "x87 FP error",
    "alignment check",
    "machine check",
    "SIMD FP error",
    "virtualization",
    "control protection",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "hypervisor injection",
    "VMM communication",
    "security",
    "reserved",
];

pub const EXCEPTION_SHORT: [&str; 32] = [
    "#DE", "#DB", "NMI", "#BP", "#OF", "#BR", "#UD", "#NM", "#DF", "#CSO", "#TS", "#NP", "#SS",
    "#GP", "#PF", "#15", "#MF", "#AC", "#MC", "#XM", "#VE", "#CP", "#22", "#23", "#24", "#25",
    "#26", "#27", "#HV", "#VC", "#SX", "#31",
];

/// Called from `trap_common` with interrupts off.
#[unsafe(no_mangle)]
extern "C" fn trap_dispatch(frame: &mut TrapFrame) {
    match frame.vector {
        0..=31 => crate::proc::fault::on_exception(frame),
        32 => crate::proc::sched::on_timer(frame),
        33 => crate::drivers::keyboard::on_irq(),
        39 | 47 => crate::arch::pic::spurious(frame.vector as u8),
        34..=46 => crate::arch::pic::eoi((frame.vector - 32) as u8),
        0x80 => crate::syscall::dispatch(frame),
        v => panic!("unexpected trap vector {v}"),
    }
}
