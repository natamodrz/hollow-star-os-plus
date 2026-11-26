//! Minimal IRQ-safe synchronization primitives for Delta Kernel.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch;

/// Spinlock that masks local CPU IRQs while held to avoid single-core or ISR deadlocks.
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: Access to `data` is serialized by `locked` with local IRQs masked.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}
// SAFETY: Sending an `IrqSpinLock<T>` transfers ownership of `T` across threads/cores.
unsafe impl<T: Send> Send for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    /// Creates a new unlocked `IrqSpinLock`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Masks local IRQs, acquires the spinlock, and returns an RAII guard.
    pub fn lock(&self) -> IrqSpinGuard<'_, T> {
        let saved_daif = arch::save_and_disable_irqs();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        IrqSpinGuard {
            lock: self,
            saved_daif,
        }
    }
}

/// RAII guard restoring lock state and previous `DAIF` IRQ mask on drop.
pub struct IrqSpinGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    saved_daif: u64,
}

impl<T> Deref for IrqSpinGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Holding `IrqSpinGuard` guarantees exclusive access to `data`.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for IrqSpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Holding `IrqSpinGuard` guarantees exclusive mutable access to `data`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IrqSpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        arch::restore_irqs(self.saved_daif);
    }
}
