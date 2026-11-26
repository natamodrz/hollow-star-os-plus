// AArch64 EL1 Exception Vector Table (VBAR_EL1).
// Each of the 16 entries must be 128-byte aligned inside a 2048-byte aligned table.

.macro VECTOR_ENTRY kind, target
    .balign 128
    sub sp, sp, #288
    stp x0, x1, [sp, #0]
    mov x1, #\kind
    b   \target
.endm

.macro SAVE_REGS
    stp x2,  x3,  [sp, #16]
    stp x4,  x5,  [sp, #32]
    stp x6,  x7,  [sp, #48]
    stp x8,  x9,  [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    mrs x2, elr_el1
    stp x30, x2,  [sp, #240]
    mrs x2, spsr_el1
    mrs x3, esr_el1
    stp x2,  x3,  [sp, #256]
    mrs x4, far_el1
    str x4,       [sp, #272]
    mov x0, sp
.endm

.macro RESTORE_REGS_AND_ERET
    ldr x2, [sp, #248]
    ldr x3, [sp, #256]
    msr elr_el1, x2
    msr spsr_el1, x3
    ldp x2,  x3,  [sp, #16]
    ldp x4,  x5,  [sp, #32]
    ldp x6,  x7,  [sp, #48]
    ldp x8,  x9,  [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30,      [sp, #240]
    ldp x0,  x1,  [sp, #0]
    add sp, sp, #288
    eret
.endm

.section .text
.balign 2048
.global exception_vector_table

exception_vector_table:
    // Current EL with SP0
    VECTOR_ENTRY 0,  exc_common_sync
    VECTOR_ENTRY 1,  exc_common_irq
    VECTOR_ENTRY 2,  exc_common_unhandled
    VECTOR_ENTRY 3,  exc_common_unhandled

    // Current EL with SPx (EL1h)
    VECTOR_ENTRY 4,  exc_common_sync
    VECTOR_ENTRY 5,  exc_common_irq
    VECTOR_ENTRY 6,  exc_common_unhandled
    VECTOR_ENTRY 7,  exc_common_unhandled

    // Lower EL using AArch64 (EL0)
    VECTOR_ENTRY 8,  exc_common_sync
    VECTOR_ENTRY 9,  exc_common_irq
    VECTOR_ENTRY 10, exc_common_unhandled
    VECTOR_ENTRY 11, exc_common_unhandled

    // Lower EL using AArch32
    VECTOR_ENTRY 12, exc_common_unhandled
    VECTOR_ENTRY 13, exc_common_unhandled
    VECTOR_ENTRY 14, exc_common_unhandled
    VECTOR_ENTRY 15, exc_common_unhandled

.balign 128

exc_common_sync:
    SAVE_REGS
    bl rust_sync_exception_handler
    RESTORE_REGS_AND_ERET

exc_common_irq:
    SAVE_REGS
    bl rust_irq_handler
    RESTORE_REGS_AND_ERET

exc_common_unhandled:
    SAVE_REGS
    bl rust_unhandled_exception_handler
    RESTORE_REGS_AND_ERET
