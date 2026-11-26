/*
 * Linker script for Raspberry Pi 3B (BCM2837 / AArch64 kernel8.img).
 * Firmware and QEMU `-M raspi3b` load raw AArch64 kernel images at 0x80000.
 */

ENTRY(_start)

SECTIONS
{
    . = 0x80000;

    __kernel_start = .;
    __text_start = .;

    .text : ALIGN(4096) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }
    . = ALIGN(4096);
    __text_end = .;

    __rodata_start = .;
    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }
    . = ALIGN(4096);
    __rodata_end = .;

    __rw_start = .;
    .data : ALIGN(4096) {
        *(.data .data.*)
    }

    . = ALIGN(16);
    __bss_start = .;
    .bss (NOLOAD) : {
        *(.bss .bss.*)
        *(COMMON)
    }
    . = ALIGN(16);
    __bss_end = .;

    /* Dedicated NOLOAD boot stack placed after BSS so clearing BSS never
       overwrites the active stack and stack growth stays above 0x80000. */
    . = ALIGN(4096);
    .stack (NOLOAD) : {
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
