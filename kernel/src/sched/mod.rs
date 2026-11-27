//! Preemptive SMP Delta Scheduler (BeOS-inspired priority queues + per-process TTBR0).

pub mod priority;

use core::sync::atomic::{AtomicU8, Ordering};

use hal_core::{PAGE_SIZE, PhysAddr};

use crate::arch;
use crate::arch::context::{CpuContext, switch_context};
use crate::arch::mmu;
use crate::mem::{phys_to_virt, pmm};
use crate::sync::IrqSpinLock;

pub use priority::Priority;

pub const MAX_CORES: usize = 4;
pub const MAX_THREADS: usize = 64;
const STACK_PAGES_PER_THREAD: usize = 4; // 16 KiB EL1 kernel stack

/// Lifecycle state of a kernel or EL0 user thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Unused,
    Ready,
    Running,
    Blocked,
    Exited,
}

/// Thread identifier (`1..`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadId(pub u64);

/// Errors returned when spawning a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    TableFull,
    OutOfStackMemory,
}

#[derive(Clone, Copy)]
struct ThreadControlBlock {
    id: u64,
    pid: u64,
    name: &'static str,
    state: ThreadState,
    base_priority: Priority,
    effective_priority: Priority,
    quantum_remaining: u32,
    affinity: Option<u8>,
    enqueue_seq: u64,
    ticks_executed: u64,
    is_idle: bool,
    ttbr0_root: PhysAddr,
    cpu_context: CpuContext,
    stack_base: Option<PhysAddr>,
}

impl ThreadControlBlock {
    const fn unused() -> Self {
        Self {
            id: 0,
            pid: 0,
            name: "",
            state: ThreadState::Unused,
            base_priority: Priority::IDLE,
            effective_priority: Priority::IDLE,
            quantum_remaining: 0,
            affinity: None,
            enqueue_seq: 0,
            ticks_executed: 0,
            is_idle: false,
            ttbr0_root: PhysAddr(0),
            cpu_context: CpuContext::empty(),
            stack_base: None,
        }
    }
}

struct SchedulerState {
    threads: [ThreadControlBlock; MAX_THREADS],
    current_slot: [Option<usize>; MAX_CORES],
    idle_slot: [Option<usize>; MAX_CORES],
    pending_requeue: [Option<usize>; MAX_CORES],
    next_id: u64,
    next_seq: u64,
    context_switches: u64,
}

impl SchedulerState {
    const fn new() -> Self {
        Self {
            threads: [const { ThreadControlBlock::unused() }; MAX_THREADS],
            current_slot: [None; MAX_CORES],
            idle_slot: [None; MAX_CORES],
            pending_requeue: [None; MAX_CORES],
            next_id: 1,
            next_seq: 1,
            context_switches: 0,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        self.threads
            .iter()
            .position(|t| t.state == ThreadState::Unused)
    }

    fn register_idle_core(&mut self, core: usize, name: &'static str) {
        if core >= MAX_CORES || self.idle_slot[core].is_some() {
            return;
        }
        let slot = self.alloc_slot().expect("idle thread slot must exist");
        let id = self.next_id;
        self.next_id += 1;
        self.threads[slot] = ThreadControlBlock {
            id,
            pid: 0,
            name,
            state: ThreadState::Running,
            base_priority: Priority::IDLE,
            effective_priority: Priority::IDLE,
            quantum_remaining: 1,
            affinity: Some(core as u8),
            enqueue_seq: 0,
            ticks_executed: 0,
            is_idle: true,
            ttbr0_root: mmu::kernel_idle_ttbr0(),
            cpu_context: CpuContext::empty(),
            stack_base: None,
        };
        self.idle_slot[core] = Some(slot);
        self.current_slot[core] = Some(slot);
    }

    /// Picks the highest-priority `Ready` thread runnable on `core`.
    fn pick_next_ready(&self, core: usize) -> Option<usize> {
        let mut best_slot: Option<usize> = None;
        let mut best_prio = Priority::IDLE;
        let mut best_seq = u64::MAX;

        for (idx, t) in self.threads.iter().enumerate() {
            if t.state != ThreadState::Ready || t.is_idle {
                continue;
            }
            if t.affinity.is_some_and(|a| usize::from(a) != core) {
                continue;
            }
            let better = match best_slot {
                None => true,
                Some(_) => {
                    t.effective_priority > best_prio
                        || (t.effective_priority == best_prio && t.enqueue_seq < best_seq)
                }
            };
            if better {
                best_slot = Some(idx);
                best_prio = t.effective_priority;
                best_seq = t.enqueue_seq;
            }
        }
        best_slot
    }

    fn finish_pending_switch(&mut self, core: usize) {
        if core >= MAX_CORES {
            return;
        }
        if let Some(prev_slot) = self.pending_requeue[core].take() {
            let prev = &mut self.threads[prev_slot];
            if prev.state == ThreadState::Running && !prev.is_idle {
                prev.state = ThreadState::Ready;
                prev.enqueue_seq = self.next_seq;
                self.next_seq += 1;
            } else if prev.is_idle {
                prev.state = ThreadState::Ready;
            } else if prev.state == ThreadState::Exited
                && !prev.is_idle
                && let Some(stack_phys) = prev.stack_base.take()
            {
                // Reclaim exited thread kernel stack now that `core` has switched off it.
                for i in 0..STACK_PAGES_PER_THREAD {
                    let _ = pmm::free_frame(PhysAddr(stack_phys.as_usize() + i * PAGE_SIZE));
                }
                prev.state = ThreadState::Unused;
            }
        }
    }
}

static SCHED: IrqSpinLock<SchedulerState> = IrqSpinLock::new(SchedulerState::new());
static ACTIVE_SCHED_CORES: AtomicU8 = AtomicU8::new(0);

/// Summary snapshot of scheduler state.
#[derive(Debug, Clone, Copy)]
pub struct SchedStats {
    pub total_threads_created: u64,
    pub context_switches: u64,
    pub active_cores_mask: u8,
}

/// Initializes the scheduler for primary Core 0 (attaching `kernel_main` as Core 0's idle thread).
pub fn init_primary() {
    SCHED.lock().register_idle_core(0, "idle-0");
    ACTIVE_SCHED_CORES.fetch_or(1 << 0, Ordering::Release);
}

/// Registers a woken secondary core (`1..=3`) with its own idle thread context.
pub fn register_secondary(core: usize) {
    let name = match core {
        1 => "idle-1",
        2 => "idle-2",
        3 => "idle-3",
        _ => "idle-x",
    };
    SCHED.lock().register_idle_core(core, name);
    if core < MAX_CORES {
        ACTIVE_SCHED_CORES.fetch_or(1 << core, Ordering::Release);
    }
}

/// Spawns a new EL1 kernel thread at `priority`.
pub fn spawn(
    name: &'static str,
    priority: Priority,
    affinity: Option<u8>,
    entry: fn(usize),
    arg: usize,
) -> Result<ThreadId, SpawnError> {
    spawn_with_space(
        name,
        0,
        mmu::kernel_idle_ttbr0(),
        priority,
        affinity,
        entry,
        arg,
    )
}

/// Spawns a thread attached to an EL0 process (`pid` + `ttbr0_root`).
pub fn spawn_user(
    name: &'static str,
    pid: u64,
    ttbr0_root: PhysAddr,
    priority: Priority,
    entry: fn(usize),
    arg: usize,
) -> Result<ThreadId, SpawnError> {
    spawn_with_space(name, pid, ttbr0_root, priority, None, entry, arg)
}

fn spawn_with_space(
    name: &'static str,
    pid: u64,
    ttbr0_root: PhysAddr,
    priority: Priority,
    affinity: Option<u8>,
    entry: fn(usize),
    arg: usize,
) -> Result<ThreadId, SpawnError> {
    let stack_phys =
        pmm::alloc_contiguous_frames(STACK_PAGES_PER_THREAD).ok_or(SpawnError::OutOfStackMemory)?;
    let stack_virt_base = phys_to_virt(stack_phys);
    let stack_top = stack_virt_base + STACK_PAGES_PER_THREAD * PAGE_SIZE;

    let id = {
        let mut sched = SCHED.lock();
        let Some(slot) = sched.alloc_slot() else {
            drop(sched);
            for i in 0..STACK_PAGES_PER_THREAD {
                let _ = pmm::free_frame(PhysAddr(stack_phys.as_usize() + i * PAGE_SIZE));
            }
            return Err(SpawnError::TableFull);
        };

        let id = sched.next_id;
        let seq = sched.next_seq;
        sched.next_id += 1;
        sched.next_seq += 1;

        sched.threads[slot] = ThreadControlBlock {
            id,
            pid,
            name,
            state: ThreadState::Ready,
            base_priority: priority,
            effective_priority: priority,
            quantum_remaining: priority.default_quantum(),
            affinity,
            enqueue_seq: seq,
            ticks_executed: 0,
            is_idle: false,
            ttbr0_root,
            cpu_context: CpuContext::init_thread(entry, arg, stack_top),
            stack_base: Some(stack_phys),
        };
        id
    };

    arch::send_event();
    Ok(ThreadId(id))
}

/// Returns `(tid, pid, ttbr0_root)` for the currently executing thread on this core.
pub fn current_thread_info() -> (ThreadId, u64, PhysAddr) {
    let core = usize::from(arch::core_id());
    let sched = SCHED.lock();
    if let Some(slot) = sched.current_slot[core] {
        let t = &sched.threads[slot];
        (ThreadId(t.id), t.pid, t.ttbr0_root)
    } else {
        (ThreadId(0), 0, mmu::kernel_idle_ttbr0())
    }
}

/// Applies a temporary Priority Inheritance boost to `thread_id`.
pub fn boost_priority(thread_id: ThreadId, min_priority: Priority) {
    let mut sched = SCHED.lock();
    for t in &mut sched.threads {
        if t.id == thread_id.0
            && t.state != ThreadState::Unused
            && min_priority > t.effective_priority
        {
            t.effective_priority = min_priority;
            break;
        }
    }
}

/// Restores a thread's effective priority back to its base priority.
pub fn clear_priority_boost(thread_id: ThreadId) {
    let mut sched = SCHED.lock();
    for t in &mut sched.threads {
        if t.id == thread_id.0 && t.state != ThreadState::Unused {
            t.effective_priority = t.base_priority;
            break;
        }
    }
}

/// Returns the effective priority of `thread_id` if active.
pub fn effective_priority(thread_id: ThreadId) -> Option<Priority> {
    let sched = SCHED.lock();
    sched
        .threads
        .iter()
        .find(|t| t.id == thread_id.0 && t.state != ThreadState::Unused)
        .map(|t| t.effective_priority)
}

/// Returns the thread name and base priority of `thread_id` if active.
pub fn thread_metadata(thread_id: ThreadId) -> Option<(&'static str, Priority)> {
    let sched = SCHED.lock();
    sched
        .threads
        .iter()
        .find(|t| t.id == thread_id.0 && t.state != ThreadState::Unused)
        .map(|t| (t.name, t.base_priority))
}

/// Blocks the calling thread until [`unblock`] wakes it.
pub fn block_current() {
    let core = usize::from(arch::core_id());
    {
        let mut sched = SCHED.lock();
        if let Some(slot) = sched.current_slot[core]
            && !sched.threads[slot].is_idle
        {
            sched.threads[slot].state = ThreadState::Blocked;
        }
    }
    schedule_on_core(core, false);
}

/// Marks a previously blocked thread as `Ready` and broadcasts a wakeup event.
pub fn unblock(thread_id: ThreadId) {
    {
        let mut sched = SCHED.lock();
        let next_seq = sched.next_seq;
        for t in &mut sched.threads {
            if t.id == thread_id.0 && t.state == ThreadState::Blocked {
                t.state = ThreadState::Ready;
                t.enqueue_seq = next_seq;
                sched.next_seq += 1;
                break;
            }
        }
    }
    arch::send_event();
}

/// Marks the calling thread as `Exited` and permanently schedules away.
pub fn exit_current() -> ! {
    sched_exit_current_thread()
}

/// Voluntarily yields the current core so higher- or equal-priority threads can run.
pub fn yield_now() {
    let core = usize::from(arch::core_id());
    schedule_on_core(core, false);
}

/// Called from the timer IRQ handler on `core` to update ticks and preempt if needed.
pub fn on_timer_tick(core: usize) {
    if core >= MAX_CORES {
        return;
    }

    let need_resched = {
        let mut sched = SCHED.lock();
        let Some(cur_slot) = sched.current_slot[core] else {
            return;
        };
        let cur_prio = {
            let cur = &mut sched.threads[cur_slot];
            cur.ticks_executed += 1;
            if !cur.effective_priority.is_realtime() && cur.quantum_remaining > 0 {
                cur.quantum_remaining -= 1;
            }
            (cur.is_idle, cur.quantum_remaining, cur.effective_priority)
        };

        if let Some(next_slot) = sched.pick_next_ready(core) {
            let next_prio = sched.threads[next_slot].effective_priority;
            cur_prio.0 || next_prio > cur_prio.2 || (next_prio == cur_prio.2 && cur_prio.1 == 0)
        } else {
            false
        }
    };

    if need_resched {
        schedule_on_core(core, true);
    }
}

/// Returns a snapshot of scheduler activity metrics.
pub fn stats() -> SchedStats {
    let sched = SCHED.lock();
    SchedStats {
        total_threads_created: sched.next_id.saturating_sub(1),
        context_switches: sched.context_switches,
        active_cores_mask: ACTIVE_SCHED_CORES.load(Ordering::Acquire),
    }
}

fn schedule_on_core(core: usize, from_irq: bool) {
    if core >= MAX_CORES {
        return;
    }

    let saved_daif = arch::save_and_disable_irqs();
    let switch_plan: Option<(*mut CpuContext, *const CpuContext, PhysAddr, PhysAddr)> = {
        let mut sched = SCHED.lock();
        if sched.pending_requeue[core].is_some() {
            None
        } else if let Some(prev_slot) = sched.current_slot[core] {
            let prev_is_idle = sched.threads[prev_slot].is_idle;
            let prev_state = sched.threads[prev_slot].state;
            let prev_prio = sched.threads[prev_slot].effective_priority;
            let prev_quantum = sched.threads[prev_slot].quantum_remaining;

            let ready_candidate = sched.pick_next_ready(core);
            let next_slot = match ready_candidate {
                Some(candidate) => {
                    let cand_prio = sched.threads[candidate].effective_priority;
                    if prev_state == ThreadState::Exited
                        || prev_state == ThreadState::Blocked
                        || prev_is_idle
                        || cand_prio > prev_prio
                        || (cand_prio == prev_prio && (prev_quantum == 0 || !from_irq))
                    {
                        candidate
                    } else {
                        prev_slot
                    }
                }
                None => {
                    if prev_state == ThreadState::Exited || prev_state == ThreadState::Blocked {
                        sched.idle_slot[core].unwrap_or(prev_slot)
                    } else {
                        prev_slot
                    }
                }
            };

            if next_slot == prev_slot {
                None
            } else {
                let next_prio = sched.threads[next_slot].effective_priority;
                let prev_ttbr0 = sched.threads[prev_slot].ttbr0_root;
                let next_ttbr0 = sched.threads[next_slot].ttbr0_root;

                sched.threads[next_slot].state = ThreadState::Running;
                sched.threads[next_slot].quantum_remaining = next_prio.default_quantum();
                sched.current_slot[core] = Some(next_slot);
                sched.pending_requeue[core] = Some(prev_slot);
                sched.context_switches += 1;

                let prev_ctx = &raw mut sched.threads[prev_slot].cpu_context;
                let next_ctx = &raw const sched.threads[next_slot].cpu_context;
                Some((prev_ctx, next_ctx, prev_ttbr0, next_ttbr0))
            }
        } else {
            None
        }
    };

    if let Some((prev_ctx, next_ctx, prev_ttbr0, next_ttbr0)) = switch_plan {
        if next_ttbr0 != prev_ttbr0 {
            // SAFETY: Kernel runs purely in `TTBR1_EL1`; switching `TTBR0_EL1` safely swaps
            // the EL0 lower-half address space while IRQs are masked.
            unsafe {
                mmu::switch_ttbr0(next_ttbr0);
            }
        }
        // SAFETY: `SCHED` lock is released with local IRQs masked (`saved_daif` keeps I=1);
        // `prev_slot` stays `Running`/`Blocked`/`Exited` in `pending_requeue[core]` until
        // `finish_pending_switch`, preventing stack aliasing across cores.
        unsafe {
            switch_context(prev_ctx, next_ctx);
        }
        sched_finish_switch_current_core();
    }

    if !from_irq {
        arch::restore_irqs(saved_daif);
    }
}

/// Finalizes the deferred requeue of the outgoing thread on the current CPU core.
#[unsafe(no_mangle)]
pub extern "C" fn sched_finish_switch_current_core() {
    let core = usize::from(arch::core_id());
    SCHED.lock().finish_pending_switch(core);
    arch::send_event();
}

/// Marks the currently executing thread as `Exited` and switches away forever.
#[unsafe(no_mangle)]
pub extern "C" fn sched_exit_current_thread() -> ! {
    let core = usize::from(arch::core_id());
    let _ = arch::save_and_disable_irqs();
    {
        let mut sched = SCHED.lock();
        if let Some(slot) = sched.current_slot[core]
            && !sched.threads[slot].is_idle
        {
            sched.threads[slot].state = ThreadState::Exited;
        }
    }
    loop {
        schedule_on_core(core, false);
        arch::wait_for_event();
    }
}
