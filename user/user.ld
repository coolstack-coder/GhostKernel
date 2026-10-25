/* User programs live at 1 GiB, inside the PDPT slot the kernel leaves
   empty for them. Code and data sit on separate pages so code can be
   mapped read-only. */
ENTRY(_start)

SECTIONS
{
    . = 0x40000000;
    .text : ALIGN(4K)   { *(.text._start) *(.text .text.*) }
    .rodata : ALIGN(4K) { *(.rodata .rodata.*) }
    .data : ALIGN(4K)   { *(.data .data.*) *(.got .got.*) }
    .bss : ALIGN(4K)    { *(.bss .bss.*) *(COMMON) }
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
