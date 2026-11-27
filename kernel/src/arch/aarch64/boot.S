// AArch64 higher-half early boot entry for Delta Kernel (Raspberry Pi 3B / Cortex-A53).
// Primary core enters `_start` at physical 0x80000 with x0 = DTB physical address (or 0).
// Boot assembly drops EL3/EL2 -> EL1, zeros .bss, brings up an initial boot TTBR0/TTBR1
// page table, enables MMU, and jumps to higher-half `kernel_main` (0xFFFF000000080000+).

.section .text.boot, "ax"
.global _start
.global _start_secondary

_start:
    // Preserve firmware/QEMU DTB pointer across early EL setup.
    mov x19, x0

    // Park any unexpected secondary core entering `_start`; SMP wakeup uses `_start_secondary`.
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
    mov x0, #0x581
    msr scr_el3, x0
    mov x0, #0x3C9
    msr spsr_el3, x0
    adr x0, .Lfrom_el2
    msr elr_el3, x0
    isb
    eret

.Lfrom_el2:
    mov x0, #(1 << 31)
    msr hcr_el2, x0

    mrs x0, cnthctl_el2
    orr x0, x0, #0x3
    msr cnthctl_el2, x0
    msr cntvoff_el2, xzr

    mrs x0, cptr_el2
    bic x0, x0, #(1 << 10)
    bic x0, x0, #(1 << 31)
    msr cptr_el2, x0

    mov x0, #0x3C5
    msr spsr_el2, x0
    adr x0, .Lel1_ready
    msr elr_el2, x0
    isb
    eret

.Lel1_ready:
    // Enable FP/SIMD instructions in EL1 (CPACR_EL1.FPEN = 0b11).
    mov x0, #(0x3 << 20)
    msr cpacr_el1, x0
    isb

    // Clear .bss (including BOOT_PT_* pages) using PC-relative physical addresses.
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

    // Populate initial 2 MiB-block boot page tables (covers 0x0..0x4020_0000 for both TTBR0 & TTBR1).
    adrp x0, BOOT_PT_L0
    add  x0, x0, :lo12:BOOT_PT_L0
    adrp x1, BOOT_PT_L1
    add  x1, x1, :lo12:BOOT_PT_L1
    adrp x2, BOOT_PT_L2_LOW
    add  x2, x2, :lo12:BOOT_PT_L2_LOW
    adrp x3, BOOT_PT_L2_HIGH
    add  x3, x3, :lo12:BOOT_PT_L2_HIGH

    // L0[0] -> L1
    orr  x4, x1, #0b11
    str  x4, [x0, #0]
    // L1[0] -> L2_LOW (0..1 GiB), L1[1] -> L2_HIGH (1..2 GiB)
    orr  x4, x2, #0b11
    str  x4, [x1, #0]
    orr  x4, x3, #0b11
    str  x4, [x1, #8]

    // Fill L2_LOW[0..504] with Normal WB/WA 2 MiB blocks (0x0..0x3F00_0000).
    mov  x4, #0                     // index
    mov  x5, #504
    mov  x6, #0x701                 // Valid | Block | Attr0 | InnerShare | AF
.Ll2_ram_loop:
    lsl  x7, x4, #21                // phys = index * 2 MiB
    orr  x7, x7, x6
    str  x7, [x2, x4, lsl #3]
    add  x4, x4, #1
    cmp  x4, x5
    b.lo .Ll2_ram_loop

    // Fill L2_LOW[504..512] with Device-nGnRnE 2 MiB blocks (0x3F00_0000..0x4000_0000).
    mov  x5, #512
    ldr  x6, =0x0060000000000405    // Valid | Block | Attr1 | AF | PXN | UXN
.Ll2_mmio_loop:
    lsl  x7, x4, #21
    orr  x7, x7, x6
    str  x7, [x2, x4, lsl #3]
    add  x4, x4, #1
    cmp  x4, x5
    b.lo .Ll2_mmio_loop

    // L2_HIGH[0] -> 0x4000_0000..0x4020_0000 Device-nGnRnE (ARM Local Peripherals).
    mov  x7, #0x40000000
    orr  x7, x7, x6
    str  x7, [x3, #0]

    // Enable MMU with BOOT_PT_L0 in both TTBR0_EL1 and TTBR1_EL1.
    bl   .Lenable_boot_mmu

    // Jump into true higher-half TTBR1_EL1 virtual address space.
    ldr  x1, =__boot_stack_top
    mov  sp, x1
    ldr  x1, =kernel_main
    mov  x0, x19
    blr  x1
    b    .Lpark_core

// Enable MMU using L0 root physical address in x0 for both TTBR0_EL1 and TTBR1_EL1.
// Clobbers x1..x4; preserves x0 and x19.
.Lenable_boot_mmu:
    ldr  x1, =0x00000000004400FF    // Attr0=0xFF (Normal), Attr1=0x00 (Device), Attr2=0x44 (NC)
    msr  mair_el1, x1

    mrs  x1, id_aa64mmfr0_el1
    and  x1, x1, #0xF
    cmp  x1, #5
    b.ls .Lips_ok
    mov  x1, #5
.Lips_ok:
    ldr  x2, =0x00000000B5103510    // T0SZ=16, T1SZ=16, IRGN/ORGN=WBWA, SH=IS, TG0=4K, TG1=4K
    orr  x2, x2, x1, lsl #32
    msr  tcr_el1, x2

    msr  ttbr0_el1, x0
    msr  ttbr1_el1, x0
    dsb  ishst
    isb
    tlbi vmalle1
    ic   iallu
    dsb  ish
    isb

    mrs  x1, sctlr_el1
    ldr  x2, =0x101D                // M (0) | C (2) | SA (3) | SA0 (4) | I (12)
    orr  x1, x1, x2
    ldr  x2, =((1 << 19) | (1 << 24) | (1 << 25))
    bic  x1, x1, x2
    msr  sctlr_el1, x1
    isb
    ret

// Secondary core entry jumped to from firmware spin-table (`0xE0`/`0xE8`/`0xF0`).
_start_secondary:
    mrs x19, mpidr_el1
    and x19, x19, #0xFF
    cbz x19, .Lpark_core
    cmp x19, #4
    b.hs .Lpark_core

    mrs x0, CurrentEL
    lsr x0, x0, #2
    and x0, x0, #0x3

    cmp x0, #3
    b.eq .Lsec_from_el3
    cmp x0, #2
    b.eq .Lsec_from_el2
    b .Lsec_el1_ready

.Lsec_from_el3:
    mov x0, #0x581
    msr scr_el3, x0
    mov x0, #0x3C9
    msr spsr_el3, x0
    adr x0, .Lsec_from_el2
    msr elr_el3, x0
    isb
    eret

.Lsec_from_el2:
    mov x0, #(1 << 31)
    msr hcr_el2, x0

    mrs x0, cnthctl_el2
    orr x0, x0, #0x3
    msr cnthctl_el2, x0
    msr cntvoff_el2, xzr

    mrs x0, cptr_el2
    bic x0, x0, #(1 << 10)
    bic x0, x0, #(1 << 31)
    msr cptr_el2, x0

    mov x0, #0x3C5
    msr spsr_el2, x0
    adr x0, .Lsec_el1_ready
    msr elr_el2, x0
    isb
    eret

.Lsec_el1_ready:
    mov x0, #(0x3 << 20)
    msr cpacr_el1, x0
    isb

    adrp x0, BOOT_PT_L0
    add  x0, x0, :lo12:BOOT_PT_L0
    bl   .Lenable_boot_mmu

    ldr  x0, =.Lsec_higher_half
    br   x0

.Lsec_higher_half:
    ldr  x0, =SECONDARY_STACK_TOPS
    ldr  x0, [x0, x19, lsl #3]
    cbz  x0, .Lpark_core
    mov  sp, x0

    mov  x0, x19
    ldr  x1, =secondary_kernel_main
    blr  x1

.Lpark_core:
    wfe
    b    .Lpark_core

.section .bss
.balign 4096
BOOT_PT_L0:
    .space 4096
BOOT_PT_L1:
    .space 4096
BOOT_PT_L2_LOW:
    .space 4096
BOOT_PT_L2_HIGH:
    .space 4096
