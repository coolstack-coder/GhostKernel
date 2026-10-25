/* GhostKernel lives identity-mapped at 1 MiB. Multiboot wants its header
   inside the first 8 KiB of the image, so .boot goes first. */
ENTRY(_start)

SECTIONS
{
    . = 1M;
    __kernel_start = .;

    .boot : ALIGN(8)
    {
        KEEP(*(.multiboot))
        *(.boot.text)
    }

    .text : ALIGN(4K)   { *(.text .text.*) }
    .rodata : ALIGN(4K) { *(.rodata .rodata.*) }
    .data : ALIGN(4K)   { *(.data .data.*) *(.got .got.*) }

    .bss (NOLOAD) : ALIGN(4K)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    . = ALIGN(4K);
    __kernel_end = .;

    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
