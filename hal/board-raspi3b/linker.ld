/*
 * Linker script for Raspberry Pi 3B (BCM2837 / AArch64 kernel8.img).
 * Kernel executes in higher-half TTBR1_EL1 virtual space (0xFFFF000000080000+),
 * while firmware and QEMU `-M raspi3b` load raw physical segments at 0x80000.
 */

ENTRY(_start)

KERNEL_VMA = 0xFFFF000000000000;

SECTIONS
{
    . = 0xFFFF000000080000;

    __kernel_start = .;
    __text_start = .;

    .text : AT(ADDR(.text) - KERNEL_VMA) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }
    . = ALIGN(4096);
    __text_end = .;

    __rodata_start = .;
    .rodata : AT(ADDR(.rodata) - KERNEL_VMA) {
        *(.rodata .rodata.*)
    }
    . = ALIGN(4096);
    __rodata_end = .;

    __rw_start = .;
    .data : AT(ADDR(.data) - KERNEL_VMA) {
        *(.data .data.*)
    }

    . = ALIGN(16);
    __bss_start = .;
    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VMA) {
        *(.bss .bss.*)
        *(COMMON)
    }
    . = ALIGN(16);
    __bss_end = .;

    /* Dedicated NOLOAD boot stack placed after BSS so clearing BSS never
       overwrites the active stack. */
    . = ALIGN(4096);
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_VMA) {
        __boot_stack_bottom = .;
        . += 65536;
        . = ALIGN(16);
        __boot_stack_top = .;
    }

    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
