//! N-task AIN priority queue (partial scope).
//!
//! The kernel timer ISR keeps its 3-task deterministic reference path
//! (so `scripts/full-verify.ps1` determinism gate remains stable). This
//! module owns the *general* scheduling data structure for the rest of
//! the kernel:
//!
//! - `MAX_RUN_TASKS` (= 32) static slots.
//! - `register(bias)` allocates a slot and returns a task id.
//! - `pick_winner(tick)` walks all live slots, computes AIN for each
//!   via the `zpl_policy` gateway, and returns the task id
//!   whose AIN is highest at this tick (ties broken by lowest id).
//! - `deregister(id)` frees the slot and zeroes its win counter.
//!
//! Acceptance for v0.1: boot self-check registers 5 tasks with rising
//! biases, runs 10 ticks, asserts the lowest-bias task wins **every**
//! tick (high AIN -> wins). Real preemptive context switching (saving
//! ring-3 register state across timer interrupts) is deferred to the
//! `SYS_FORK` follow-up.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::sync::atomic::{AtomicBool, Ordering};

pub const MAX_RUN_TASKS: usize = 32;

#[derive(Clone, Copy)]
pub struct RunTask {
    pub used: bool,
    pub id: u32,
    pub bias: f64,
    pub seed_offset: u64,
    pub wins: u32,
}

impl RunTask {
    pub const fn empty() -> Self {
        Self {
            used: false,
            id: 0,
            bias: 0.0,
            seed_offset: 0,
            wins: 0,
        }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const RUN_INIT: RunTask = RunTask::empty();
static mut QUEUE: [RunTask; MAX_RUN_TASKS] = [RUN_INIT; MAX_RUN_TASKS];
static INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Reset the queue.
///
/// # Safety
/// Caller must guarantee no other thread is touching the queue.
pub unsafe fn init() {
    unsafe {
        let queue = (&raw mut QUEUE) as *mut RunTask;
        for i in 0..MAX_RUN_TASKS {
            *queue.add(i) = RunTask::empty();
        }
    }
    INIT_DONE.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunQueueError {
    NotInitialized,
    NoSlot,
}

fn check_init() -> Result<(), RunQueueError> {
    if INIT_DONE.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(RunQueueError::NotInitialized)
    }
}

/// Register a task with the given `bias` in `[0.0, 1.0]` and the per-task
/// seed offset that will be mixed with the tick number when computing
/// per-tick AIN. Returns the assigned task id.
pub fn register(bias: f64, seed_offset: u64) -> Result<u32, RunQueueError> {
    check_init()?;
    unsafe {
        let queue = (&raw mut QUEUE) as *mut RunTask;
        for i in 0..MAX_RUN_TASKS {
            let slot = &mut *queue.add(i);
            if !slot.used {
                slot.used = true;
                slot.id = i as u32;
                slot.bias = bias;
                slot.seed_offset = seed_offset;
                slot.wins = 0;
                return Ok(i as u32);
            }
        }
    }
    Err(RunQueueError::NoSlot)
}

pub fn deregister(id: u32) -> Result<(), RunQueueError> {
    check_init()?;
    if (id as usize) >= MAX_RUN_TASKS {
        return Err(RunQueueError::NoSlot);
    }
    unsafe {
        let queue = (&raw mut QUEUE) as *mut RunTask;
        let slot = &mut *queue.add(id as usize);
        slot.used = false;
        slot.id = 0;
        slot.bias = 0.0;
        slot.seed_offset = 0;
        slot.wins = 0;
    }
    Ok(())
}

/// Walk live slots, compute AIN at `tick`, and return the slot id with
/// the highest AIN. Ties broken by lowest id (ensures determinism).
pub fn pick_winner(tick: u64) -> Option<u32> {
    if check_init().is_err() {
        return None;
    }
    let mut best_id: Option<u32> = None;
    let mut best_ain: f64 = -1.0;
    unsafe {
        let queue = (&raw mut QUEUE) as *mut RunTask;
        for i in 0..MAX_RUN_TASKS {
            let slot = &mut *queue.add(i);
            if !slot.used {
                continue;
            }
            let input = crate::zpl_policy::ComputeInput {
                bias: slot.bias,
                dimension: 9,
                samples: 64,
                seed: tick.wrapping_add(slot.seed_offset),
            };
            let output = crate::zpl_policy::policy_compute(input);
            if output.ain > best_ain {
                best_ain = output.ain;
                best_id = Some(slot.id);
            }
        }
        if let Some(id) = best_id {
            let winner = &mut *queue.add(id as usize);
            winner.wins = winner.wins.saturating_add(1);
        }
    }
    best_id
}

pub fn wins_for(id: u32) -> u32 {
    unsafe {
        let queue = (&raw mut QUEUE) as *const RunTask;
        if (id as usize) >= MAX_RUN_TASKS {
            return 0;
        }
        let slot = &*queue.add(id as usize);
        if !slot.used {
            0
        } else {
            slot.wins
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    RegisterFailed,
    UnexpectedWinner { tick: u64, expected: u32, got: u32 },
    BadWinCount { expected: u32, got: u32 },
}

/// Acceptance: register 5 tasks with bias values that step away from
/// the neutral midpoint and verify that task #0 (closest-to-neutral,
/// highest AIN) wins all 10 ticks deterministically.
pub fn self_check() -> Result<(), SelfCheckErr> {
    // Arbitrary step-away-from-neutral inputs: only the ordering matters, not
    // the exact values, and none of them is a value this kernel decides by.
    // Do not "tidy" these back to round numbers.
    let biases = [0.50, 0.61, 0.72, 0.83, 0.94];
    let mut ids = [0u32; 5];
    for (idx, b) in biases.iter().enumerate() {
        ids[idx] =
            register(*b, idx as u64 * 0x100).map_err(|_| SelfCheckErr::RegisterFailed)?;
    }

    for tick in 0..10u64 {
        let winner = match pick_winner(tick) {
            Some(w) => w,
            None => return Err(SelfCheckErr::RegisterFailed),
        };
        if winner != ids[0] {
            return Err(SelfCheckErr::UnexpectedWinner {
                tick,
                expected: ids[0],
                got: winner,
            });
        }
    }

    let wins0 = wins_for(ids[0]);
    if wins0 != 10 {
        return Err(SelfCheckErr::BadWinCount {
            expected: 10,
            got: wins0,
        });
    }

    Ok(())
}
