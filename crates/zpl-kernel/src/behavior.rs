//! v0.5 step 1 — behaviour translator.
//!
//! Until v0.4 the scheduler's decision input was a constant written into the task table
//! (`interrupts.rs`, `SCHED_TASKS`). A decision computed from a number we chose ourselves
//! says nothing about the task. This module replaces that constant with bits actually
//! observed while the kernel runs.
//!
//! **What is genuinely measured today, and what is not.** The plan asks for four signals:
//! quantum consumed, syscalls, blocked/ready transitions, and interrupts. Two of them have
//! nothing to observe yet, because no task in this kernel executes code: the entries in
//! `SCHED_TASKS` and `runqueue::RunTask` are table rows that the timer ISR evaluates, and
//! ring-3 context switching is still gated on `SYS_FORK`. So:
//!
//! | field | state |
//! |---|---|
//! | [`TaskSignals::scheduled`] | **measured** — how often the scheduler picked this task |
//! | [`TaskSignals::interrupts`] | **measured** — timer ticks and page faults in the window |
//! | [`TaskSignals::syscalls`] | wired, but stays 0 until something in ring 3 issues one |
//! | [`TaskSignals::quantum`] / [`TaskSignals::blocked`] | wired, but structurally 0 until tasks execute |
//!
//! The two unmeasured fields are kept so the shape is complete and so the call sites do
//! not change when `SYS_FORK` lands. They are **not** filled with invented values, and
//! nothing here should be described as "four signals measured per tick" while they read
//! zero.
//!
//! Compiled only under the `v05_translator` feature; the default `zpl-kernel-bin` build
//! that every gate runs against is untouched.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// How many scheduler ticks one observation window spans.
///
/// Short enough that a task changing behaviour shows up quickly, long enough that a
/// single tick cannot swing the input. Its effect is measured, not assumed.
pub const WINDOW_TICKS: u64 = 16;

/// Number of scheduler slots the translator tracks. Matches `SCHED_TASKS`.
pub const MAX_TRACKED: usize = 8;

/// Raw counters for one task, as observed over the current window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskSignals {
    /// Times the scheduler picked this task during the window. **Measured.**
    pub scheduled: u32,
    /// Syscalls attributed to this task. Wired; zero until ring 3 issues syscalls.
    pub syscalls: u32,
    /// Blocked/ready transitions. Wired; zero until tasks actually execute and block.
    pub blocked: u32,
    /// Timer-tick quantum consumed. Wired; zero until tasks actually execute.
    pub quantum: u32,
    /// Interrupts observed during the window. **Measured** (timer + page faults).
    pub interrupts: u32,
}

impl TaskSignals {
    /// True when at least one field carries a genuinely measured, non-zero observation.
    #[must_use]
    pub fn has_observation(&self) -> bool {
        self.scheduled > 0 || self.interrupts > 0
    }
}

/// Per-task counters for the window in progress.
static SCHEDULED: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];
static SYSCALLS: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];
static BLOCKED: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];
static QUANTUM: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];

/// Counters for the window that has just closed — what `signals_for` reports.
static LAST_SCHEDULED: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];
static LAST_SYSCALLS: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];
static LAST_BLOCKED: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];
static LAST_QUANTUM: [AtomicU32; MAX_TRACKED] = [const { AtomicU32::new(0) }; MAX_TRACKED];

/// Kernel-wide interrupt counters. Not per task: an interrupt is not attributable to a
/// task while no task is executing. Reported to every task's window identically, and
/// labelled as such rather than pretended to be per-task.
static IRQ_TIMER: AtomicU64 = AtomicU64::new(0);
static IRQ_PAGE_FAULT: AtomicU64 = AtomicU64::new(0);
static IRQ_SYSCALL: AtomicU64 = AtomicU64::new(0);
static LAST_IRQ_TOTAL: AtomicU32 = AtomicU32::new(0);
static WINDOW_IRQ_BASE: AtomicU64 = AtomicU64::new(0);

/// Windows completed so far.
static WINDOWS: AtomicU64 = AtomicU64::new(0);
/// How many tasks the current window actually tracked, for the fair-share baseline.
static ACTIVE_TASKS: AtomicU32 = AtomicU32::new(0);

/// Record that the scheduler picked `idx` on this tick. **Measured signal.**
pub fn note_scheduled(idx: usize) {
    if idx < MAX_TRACKED {
        SCHEDULED[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Record a syscall attributed to `idx`. Currently never called from ring 3.
pub fn note_syscall(idx: usize) {
    if idx < MAX_TRACKED {
        SYSCALLS[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Record a blocked/ready transition for `idx`. Currently unreachable — see module docs.
pub fn note_blocked(idx: usize) {
    if idx < MAX_TRACKED {
        BLOCKED[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Record quantum consumed by `idx`. Currently unreachable — see module docs.
pub fn note_quantum(idx: usize, ticks: u32) {
    if idx < MAX_TRACKED {
        QUANTUM[idx].fetch_add(ticks, Ordering::Relaxed);
    }
}

/// Count one timer interrupt. **Measured signal.**
pub fn note_irq_timer() {
    IRQ_TIMER.fetch_add(1, Ordering::Relaxed);
}

/// Count one page fault. **Measured signal.**
pub fn note_irq_page_fault() {
    IRQ_PAGE_FAULT.fetch_add(1, Ordering::Relaxed);
}

/// Count one syscall interrupt. **Measured signal** (zero without ring-3 demos).
pub fn note_irq_syscall() {
    IRQ_SYSCALL.fetch_add(1, Ordering::Relaxed);
}

fn irq_total() -> u64 {
    IRQ_TIMER
        .load(Ordering::Relaxed)
        .wrapping_add(IRQ_PAGE_FAULT.load(Ordering::Relaxed))
        .wrapping_add(IRQ_SYSCALL.load(Ordering::Relaxed))
}

/// Close the window if `tick` reached its end, moving counters to the reported set.
///
/// Call once per scheduler tick, after the picks for that tick are recorded.
pub fn on_tick(tick: u64, active_tasks: u32) {
    if tick == 0 || !tick.is_multiple_of(WINDOW_TICKS) {
        return;
    }
    for i in 0..MAX_TRACKED {
        LAST_SCHEDULED[i].store(SCHEDULED[i].swap(0, Ordering::Relaxed), Ordering::Relaxed);
        LAST_SYSCALLS[i].store(SYSCALLS[i].swap(0, Ordering::Relaxed), Ordering::Relaxed);
        LAST_BLOCKED[i].store(BLOCKED[i].swap(0, Ordering::Relaxed), Ordering::Relaxed);
        LAST_QUANTUM[i].store(QUANTUM[i].swap(0, Ordering::Relaxed), Ordering::Relaxed);
    }
    let total = irq_total();
    let base = WINDOW_IRQ_BASE.swap(total, Ordering::Relaxed);
    LAST_IRQ_TOTAL.store(total.wrapping_sub(base) as u32, Ordering::Relaxed);
    ACTIVE_TASKS.store(active_tasks, Ordering::Relaxed);
    WINDOWS.fetch_add(1, Ordering::Relaxed);
}

/// Signals observed for `idx` over the window that has closed most recently.
#[must_use]
pub fn signals_for(idx: usize) -> TaskSignals {
    if idx >= MAX_TRACKED {
        return TaskSignals::default();
    }
    TaskSignals {
        scheduled: LAST_SCHEDULED[idx].load(Ordering::Relaxed),
        syscalls: LAST_SYSCALLS[idx].load(Ordering::Relaxed),
        blocked: LAST_BLOCKED[idx].load(Ordering::Relaxed),
        quantum: LAST_QUANTUM[idx].load(Ordering::Relaxed),
        interrupts: LAST_IRQ_TOTAL.load(Ordering::Relaxed),
    }
}

/// Windows closed so far.
#[must_use]
pub fn windows_closed() -> u64 {
    WINDOWS.load(Ordering::Relaxed)
}

/// Map observed signals to the gateway's input field, replacing the hand-written constant.
///
/// The mapping is deliberately the simplest thing that is defensible, so that any effect
/// seen later can be attributed to the signals rather than to a clever transform:
///
/// > a task that receives exactly its fair share of the scheduler produces the balanced
/// > value; deviation from fair share moves the input away from balanced, in proportion.
///
/// With `n` tracked tasks, fair share is `1/n` of the window. The returned value is
/// `balanced + (share - fair_share)`, clamped to `[0, 1]`.
///
/// Before the first window closes there is nothing observed, so this returns `fallback` —
/// the task's previous constant. That keeps the very first ticks identical to v0.4 instead
/// of inventing an input, and it is visible in the trace because `windows_closed()` is 0.
#[must_use]
pub fn input_for(idx: usize, fallback: f64) -> f64 {
    let s = signals_for(idx);
    map_to_input(&s, windows_closed(), ACTIVE_TASKS.load(Ordering::Relaxed), fallback)
}

/// The mapping itself, as a pure function of what was observed.
///
/// Kept free of the statics so it can be tested exhaustively and so the later three-arm
/// comparison can drive it directly with synthetic signal sets.
#[must_use]
pub fn map_to_input(
    signals: &TaskSignals,
    windows_closed: u64,
    active_tasks: u32,
    fallback: f64,
) -> f64 {
    /// The value a task receiving exactly its fair share produces.
    const BALANCED: f64 = 0.5;

    if windows_closed == 0 || !signals.has_observation() {
        return fallback;
    }
    let active = active_tasks.max(1);
    let share = f64::from(signals.scheduled) / WINDOW_TICKS as f64;
    let fair = 1.0 / f64::from(active);
    (BALANCED + (share - fair)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(scheduled: u32, interrupts: u32) -> TaskSignals {
        TaskSignals {
            scheduled,
            interrupts,
            ..TaskSignals::default()
        }
    }

    // The mapping is tested as a pure function: the crate is `no_std`, so there is no
    // mutex to serialise tests that share the counter statics, and `cargo test` runs them
    // in parallel. Testing `map_to_input` directly is both sound and more exhaustive.

    #[test]
    fn falls_back_to_the_constant_before_any_window_closes() {
        assert_eq!(map_to_input(&sig(8, 4), 0, 3, 0.48), 0.48);
    }

    #[test]
    fn falls_back_when_nothing_was_observed() {
        let empty = TaskSignals::default();
        assert!(!empty.has_observation());
        assert_eq!(map_to_input(&empty, 5, 3, 0.48), 0.48);
    }

    #[test]
    fn fair_share_maps_to_the_balanced_value() {
        // Four tasks, one window of WINDOW_TICKS, this task picked exactly a quarter.
        let quarter = (WINDOW_TICKS / 4) as u32;
        let v = map_to_input(&sig(quarter, 1), 1, 4, 0.48);
        assert!((v - 0.5).abs() < f64::EPSILON * 8.0, "expected balanced, got {v}");
    }

    #[test]
    fn a_task_that_monopolises_moves_above_balanced() {
        let all = WINDOW_TICKS as u32;
        let hog = map_to_input(&sig(all, 1), 1, 3, 0.48);
        assert!(hog > 0.5, "monopolising task should exceed balanced, got {hog}");
    }

    #[test]
    fn a_starved_task_moves_below_balanced() {
        let starved = map_to_input(&sig(0, 1), 1, 3, 0.48);
        assert!(starved < 0.5, "starved task should sit below balanced, got {starved}");
    }

    #[test]
    fn the_hog_and_the_starved_task_are_distinguished() {
        let hog = map_to_input(&sig(WINDOW_TICKS as u32, 1), 1, 3, 0.48);
        let starved = map_to_input(&sig(0, 1), 1, 3, 0.48);
        assert!(hog > starved);
    }

    #[test]
    fn the_input_stays_inside_the_unit_interval() {
        for scheduled in [0u32, 1, 7, WINDOW_TICKS as u32, WINDOW_TICKS as u32 * 10] {
            for active in [1u32, 2, 3, 8] {
                let v = map_to_input(&sig(scheduled, 1), 1, active, 0.48);
                assert!((0.0..=1.0).contains(&v), "out of range: {v}");
            }
        }
    }

    #[test]
    fn zero_active_tasks_does_not_divide_by_zero() {
        let v = map_to_input(&sig(4, 1), 1, 0, 0.48);
        assert!(v.is_finite());
        assert!((0.0..=1.0).contains(&v));
    }

    #[test]
    fn interrupts_alone_count_as_an_observation() {
        // A task never picked, in a window where interrupts were seen, is still observed:
        // "starved" is information, not absence of data.
        let s = sig(0, 3);
        assert!(s.has_observation());
        assert_ne!(map_to_input(&s, 1, 3, 0.48), 0.48);
    }

    #[test]
    fn unmeasured_fields_default_to_zero_rather_than_a_guess() {
        // Nothing in ring 3 runs yet; these must not be invented. See module docs.
        let s = TaskSignals::default();
        assert_eq!(s.syscalls, 0);
        assert_eq!(s.blocked, 0);
        assert_eq!(s.quantum, 0);
    }
}
