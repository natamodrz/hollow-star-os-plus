// AArch64 early boot entry for Delta Kernel (Raspberry Pi 3B / Cortex-A53).
// Entered at _start (0x80000) with x0 = DTB physical address (or 0 if omitted).

.section .text.boot, "ax"
.global _start

_start:
    // Preserve firmware/QEMU DTB pointer across early EL setup.
    mov x19, x0

    // Park secondary Cortex-A53 cores (MPIDR_EL1.Aff0 != 0) until M3 SMP wakeup.
    mrs x0, mpidr_el1
    and x0, x0, #0xFF
    cbnz x0, .Lpark_core

    // Inspect current exception level (CurrentEL[3:2]).
    mrs x0, CurrentEL
    lsr x0, x0, #2
    and x0, x0, #0x3

    cmp x0, #3
    b.eq .Lfrom_el3
    cmp x0, #2
    b.eq .Lfrom_el2
    b .Lel1_ready

.Lfrom_el3:
    // Drop EL3 -> EL2 (AArch64 Non-Secure) if hardware firmware starts in EL3.
    // SCR_EL3: RW (bit 10) | HCE (bit 8) | SMD (bit 7) | NS (bit 0) = 0x581
    mov x0, #0x581
    msr scr_el3, x0
    // SPSR_EL3: mask DAIF (0x3C0) | EL2h (0b1001) = 0x3C9
    mov x0, #0x3C9
    msr spsr_el3, x0
    adr x0, .Lfrom_el2
    msr elr_el3, x0
    isb
    eret

.Lfrom_el2:
    // Configure EL1 to execute in AArch64 (HCR_EL2.RW, bit 31).
    mov x0, #(1 << 31)
    msr hcr_el2, x0

    // Allow EL1 access to the physical counter/timer and clear virtual offset.
    mrs x0, cnthctl_el2
    orr x0, x0, #0x3
    msr cnthctl_el2, x0
    msr cntvoff_el2, xzr

    // Disable EL2 trapping of FP/SIMD and CPACR_EL1 while preserving RES1 bits.
    mrs x0, cptr_el2
    bic x0, x0, #(1 << 10)
    bic x0, x0, #(1 << 31)
    msr cptr_el2, x0

    // Drop EL2 -> EL1h (SP_EL1) with DAIF interrupts masked (0x3C0 | 0b0101 = 0x3C5).
    mov x0, #0x3C5
    msr spsr_el2, x0
    adr x0, .Lel1_ready
    msr elr_el2, x0
    isb
    eret

.Lel1_ready:
    // Enable FP/SIMD instructions in EL1 (CPACR_EL1.FPEN = 0b11, bits [21:20])
    // so Rust core::fmt / compiler-generated NEON ops never trap.
    mov x0, #(0x3 << 20)
    msr cpacr_el1, x0
    isb

    // Initialize EL1 stack pointer (16-byte aligned top of dedicated .stack section).
    adrp x0, __boot_stack_top
    add  x0, x0, :lo12:__boot_stack_top
    mov  sp, x0

    // Clear .bss in 16-byte steps (bounds are 16-byte aligned in linker.ld).
    adrp x0, __bss_start
    add  x0, x0, :lo12:__bss_start
    adrp x1, __bss_end
    add  x1, x1, :lo12:__bss_end
.Lbss_loop:
    cmp  x0, x1
    b.hs .Lbss_done
    stp  xzr, xzr, [x0], #16
    b    .Lbss_loop
.Lbss_done:

    // Enter Rust kernel_main(dtb_ptr: usize).
    mov  x0, x19
    bl   kernel_main

.Lpark_core:
    wfe
    b    .Lpark_core
