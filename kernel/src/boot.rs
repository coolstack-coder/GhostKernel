//! The very first instructions. QEMU's multiboot loader drops us here in
//! 32-bit protected mode with paging off; we build just enough page tables
//! to reach long mode, then call into Rust.
//!
//! Boot page tables (shared by every address space later on):
//!   PML4[0] -> PDPT
//!   PDPT[0] -> PD0  identity 0..1 GiB with 2 MiB pages (kernel, heap, frames)
//!   PDPT[3] -> PD3  identity 3..4 GiB, uncached (framebuffer + MMIO)
//! PDPT[1] (1..2 GiB) is left empty: that's where user programs live.

core::arch::global_asm!(
    r#"
.set MB_MAGIC, 0x1BADB002
.set MB_FLAGS, 0x00000003          /* page-align modules + give us a memory map */

.section .multiboot, "a"
.align 4
    .long MB_MAGIC
    .long MB_FLAGS
    .long -(MB_MAGIC + MB_FLAGS)

.section .boot.text, "ax"
.code32
.global _start
_start:
    cli
    mov esi, ebx                    /* multiboot info pointer - keep it safe */
    mov ebp, eax                    /* multiboot magic */

    /* zero .bss (page tables and the boot stack live there) */
    mov edi, offset __bss_start
    mov ecx, offset __bss_end
    sub ecx, edi
    shr ecx, 2
    xor eax, eax
    rep stosd

    mov esp, offset __boot_stack_top

    /* PML4[0] -> PDPT, PDPT[0] -> PD0, PDPT[3] -> PD3 */
    mov eax, offset boot_pdpt
    or eax, 0x3
    mov dword ptr [boot_pml4], eax
    mov eax, offset boot_pd0
    or eax, 0x3
    mov dword ptr [boot_pdpt], eax
    mov eax, offset boot_pd3
    or eax, 0x3
    mov dword ptr [boot_pdpt + 24], eax

    xor ecx, ecx
.Lfill_pd:
    mov eax, ecx
    shl eax, 21
    or eax, 0x83                    /* present | writable | 2 MiB page */
    mov dword ptr [boot_pd0 + ecx * 8], eax
    mov eax, ecx
    shl eax, 21
    add eax, 0xC0000000
    or eax, 0x9B                    /* + cache disable + write-through for MMIO */
    mov dword ptr [boot_pd3 + ecx * 8], eax
    inc ecx
    cmp ecx, 512
    jne .Lfill_pd

    /* CR4.PAE, CR3, EFER.LME, then paging on */
    mov eax, cr4
    or eax, (1 << 5)
    mov cr4, eax
    mov eax, offset boot_pml4
    mov cr3, eax
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8)
    wrmsr
    mov eax, cr0
    or eax, (1 << 31) | (1 << 16) | 1
    mov cr0, eax

    lgdt [boot_gdt_ptr]
    mov eax, offset long_mode_entry
    push 0x08
    push eax
    retf                            /* far return = far jump into 64-bit CS */

.code64
long_mode_entry:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    xor eax, eax
    mov fs, ax
    mov gs, ax
    mov rsp, offset __boot_stack_top
    mov edi, ebp                    /* arg0: magic (zero-extends) */
    mov esi, esi                    /* arg1: multiboot info */
    xor ebp, ebp
    call kmain
.Lhalt:
    hlt
    jmp .Lhalt

.section .rodata
.align 16
boot_gdt:
    .quad 0
    .quad 0x00AF9A000000FFFF        /* 64-bit kernel code */
    .quad 0x00CF92000000FFFF        /* kernel data */
boot_gdt_end:
boot_gdt_ptr:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt

.section .bss
.align 4096
.global boot_pml4
.global boot_pdpt
boot_pml4: .skip 4096
boot_pdpt: .skip 4096
boot_pd0:  .skip 4096
boot_pd3:  .skip 4096
__boot_stack_bottom: .skip 65536
.global __boot_stack_top
__boot_stack_top:
"#
);
