// AArch64 callee-saved context switch and new-thread entry trampoline.
// Must match `CpuContext` field layout in `context.rs`:
//   0x00: x19, x20
//   0x10: x21, x22
//   0x20: x23, x24
//   0x30: x25, x26
//   0x40: x27, x28
//   0x50: x29 (fp), x30 (lr)
//   0x60: sp

.section .text
.global switch_context
.global thread_entry_trampoline

switch_context:
    stp x19, x20, [x0, #0]
    stp x21, x22, [x0, #16]
    stp x23, x24, [x0, #32]
    stp x25, x26, [x0, #48]
    stp x27, x28, [x0, #64]
    stp x29, x30, [x0, #80]
    mov x9, sp
    str x9,       [x0, #96]

    ldp x19, x20, [x1, #0]
    ldp x21, x22, [x1, #16]
    ldp x23, x24, [x1, #32]
    ldp x25, x26, [x1, #48]
    ldp x27, x28, [x1, #64]
    ldp x29, x30, [x1, #80]
    ldr x9,       [x1, #96]
    mov sp, x9
    ret

thread_entry_trampoline:
    // Complete deferred requeue of the previous thread before enabling IRQs.
    bl  sched_finish_switch_current_core
    msr daifclr, #2
    isb
    mov x0, x20
    blr x19
    bl  sched_exit_current_thread
.Ltrampoline_hang:
    wfe
    b   .Ltrampoline_hang
