//! ARM Generic Timer (EL1 Non-Secure Physical Timer `CNTP`) driver.

use hal_core::Timer;

const CNTP_CTL_ENABLE: u64 = 1 << 0;
const CNTP_CTL_IMASK: u64 = 1 << 1;

/// ARMv8-A EL1 physical non-secure timer implementing [`hal_core::Timer`].
pub struct ArmGenericTimer;

impl ArmGenericTimer {
    /// Creates an ARM Generic Timer handle.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ArmGenericTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer for ArmGenericTimer {
    fn frequency_hz(&self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        {
            let freq: u64;
            // SAFETY: Reading `CNTFRQ_EL0` is always valid at EL1 and side-effect free.
            unsafe {
                core::arch::asm!("mrs {0}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
            }
            freq
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            19_200_000
        }
    }

    fn counter(&self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        {
            let cnt: u64;
            // SAFETY: `isb` orders counter reads before reading `CNTPCT_EL0` at EL1.
            unsafe {
                core::arch::asm!(
                    "isb",
                    "mrs {0}, cntpct_el0",
                    out(reg) cnt,
                    options(nomem, nostack, preserves_flags)
                );
            }
            cnt
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            0
        }
    }

    fn arm_after_ticks(&mut self, ticks: u64) {
        let clamped = ticks.min(i32::MAX as u64);
        #[cfg(target_arch = "aarch64")]
        // SAFETY: Writing `CNTP_TVAL_EL0` and `CNTP_CTL_EL0` at EL1 arms the physical
        // non-secure timer after EL2 granted access via `CNTHCTL_EL2`.
        unsafe {
            core::arch::asm!(
                "msr cntp_tval_el0, {tval}",
                "msr cntp_ctl_el0, {ctl}",
                "isb",
                tval = in(reg) clamped,
                ctl = in(reg) CNTP_CTL_ENABLE,
                options(nomem, nostack, preserves_flags)
            );
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = clamped;
    }

    fn disarm(&mut self) {
        #[cfg(target_arch = "aarch64")]
        // SAFETY: Setting `CNTP_CTL_IMASK` and clearing `ENABLE` safely masks timer IRQs.
        unsafe {
            core::arch::asm!(
                "msr cntp_ctl_el0, {ctl}",
                "isb",
                ctl = in(reg) CNTP_CTL_IMASK,
                options(nomem, nostack, preserves_flags)
            );
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = (CNTP_CTL_ENABLE, CNTP_CTL_IMASK);
    }
}
