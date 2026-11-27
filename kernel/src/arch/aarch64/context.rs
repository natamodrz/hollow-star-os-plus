//! AArch64 thread CPU context representation, switching, and EL0 drop helpers.

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(include_str!("context.S"));

/// Saved AArch64 callee-saved registers, EL1 SP, and EL0 `SP_EL0` for context switching.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct CpuContext {
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,
    pub x30: u64,
    pub sp: u64,
    pub sp_el0: u64,
}

const _: () = assert!(core::mem::size_of::<CpuContext>() == 112);

impl CpuContext {
    /// Returns a zeroed [`CpuContext`] suitable for idle or uninitialized threads.
    pub const fn empty() -> Self {
        Self {
            x19: 0,
            x20: 0,
            x21: 0,
            x22: 0,
            x23: 0,
            x24: 0,
            x25: 0,
            x26: 0,
            x27: 0,
            x28: 0,
            x29: 0,
            x30: 0,
            sp: 0,
            sp_el0: 0,
        }
    }

    /// Initializes a [`CpuContext`] for a fresh thread entering `entry(arg)` on `stack_top`.
    pub fn init_thread(entry: fn(usize), arg: usize, stack_top: usize) -> Self {
        let mut ctx = Self::empty();
        ctx.x19 = entry as usize as u64;
        ctx.x20 = arg as u64;
        ctx.x30 = (&raw const thread_entry_trampoline) as usize as u64;
        ctx.sp = (stack_top & !0xF) as u64;
        ctx
    }
}

unsafe extern "C" {
    static thread_entry_trampoline: u8;

    /// Switches execution context from `prev` to `next`.
    ///
    /// # Safety
    ///
    /// - `prev` must point to valid writable [`CpuContext`] storage for the outgoing thread.
    /// - `next` must point to a valid, initialized [`CpuContext`] whose stack is not in use
    ///   by any other core.
    /// - Local IRQs (`DAIF.I`) must be masked across the switch.
    pub fn switch_context(prev: *mut CpuContext, next: *const CpuContext);

    /// Drops the current thread from EL1 to EL0 (`EL0t`) at `entry_pc` with `user_sp`.
    ///
    /// # Safety
    ///
    /// Caller must have loaded a valid user `TTBR0_EL1` page table mapping `entry_pc`
    /// (`Code`) and the stack region below `user_sp` (`RwData`).
    pub fn enter_usermode(entry_pc: usize, user_sp: usize) -> !;
}
