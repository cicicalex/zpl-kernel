//! Process table + PID allocator + zombie reaping.
//!
//! Layout:
//! - Static slot table of `MAX_PROCESSES` entries.
//! - PIDs are u32, allocated via an atomic counter starting at 1.
//! - `spawn_zombie(parent, exit_code)` records a child that has already
//!   exited (the v0.1 substitute for ring-3 `fork()` — once user-space
//!   `fork()` lands the kernel will populate `Running` slots first and
//!   transition them to `Zombie` on `SYS_EXIT`).
//! - `wait(parent)` reaps the first zombie child of `parent`, returning
//!   `(child_pid, exit_code)` and marking the slot free.
//!
//! Boot self-check exercises the full life-cycle to satisfy
//! "create -> exit -> wait -> exit code correct".

use core::sync::atomic::{AtomicU32, Ordering};

pub const MAX_PROCESSES: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Free,
    Running,
    Zombie,
}

#[derive(Clone, Copy)]
pub struct Process {
    pub pid: u32,
    pub parent_pid: u32,
    pub state: ProcessState,
    pub exit_code: u64,
}

impl Process {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            parent_pid: 0,
            state: ProcessState::Free,
            exit_code: 0,
        }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const PROC_INIT: Process = Process::empty();
static mut TABLE: [Process; MAX_PROCESSES] = [PROC_INIT; MAX_PROCESSES];
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// Reset the table. Call once during boot.
///
/// # Safety
/// Caller must ensure no other thread is touching the table.
pub unsafe fn init() {
    unsafe {
        let table = (&raw mut TABLE) as *mut Process;
        for i in 0..MAX_PROCESSES {
            *table.add(i) = Process::empty();
        }
    }
    NEXT_PID.store(1, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcError {
    NoSlot,
    NotFound,
}

fn next_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::SeqCst)
}

/// Allocate a slot, mark it `Running`, return the new PID.
pub fn spawn(parent_pid: u32) -> Result<u32, ProcError> {
    let pid = next_pid();
    unsafe {
        let table = (&raw mut TABLE) as *mut Process;
        for i in 0..MAX_PROCESSES {
            let slot = &mut *table.add(i);
            if slot.state == ProcessState::Free {
                slot.pid = pid;
                slot.parent_pid = parent_pid;
                slot.state = ProcessState::Running;
                slot.exit_code = 0;
                return Ok(pid);
            }
        }
    }
    Err(ProcError::NoSlot)
}

/// Mark `pid`'s slot as `Zombie` carrying `exit_code`.
pub fn exit(pid: u32, exit_code: u64) -> Result<(), ProcError> {
    unsafe {
        let table = (&raw mut TABLE) as *mut Process;
        for i in 0..MAX_PROCESSES {
            let slot = &mut *table.add(i);
            if slot.state != ProcessState::Free && slot.pid == pid {
                slot.state = ProcessState::Zombie;
                slot.exit_code = exit_code;
                return Ok(());
            }
        }
    }
    Err(ProcError::NotFound)
}

/// Convenience: shortcut used by `boot::run_process_self_check` and the
/// upcoming `SYS_FORK` syscall — creates a child slot already in the
/// `Zombie` state with the given exit code.
pub fn spawn_zombie(parent_pid: u32, exit_code: u64) -> Result<u32, ProcError> {
    let pid = spawn(parent_pid)?;
    exit(pid, exit_code)?;
    Ok(pid)
}

/// Reap the first zombie child of `parent_pid`. Returns
/// `(child_pid, exit_code)` and frees the slot.
pub fn wait(parent_pid: u32) -> Option<(u32, u64)> {
    unsafe {
        let table = (&raw mut TABLE) as *mut Process;
        for i in 0..MAX_PROCESSES {
            let slot = &mut *table.add(i);
            if slot.state == ProcessState::Zombie && slot.parent_pid == parent_pid {
                let result = Some((slot.pid, slot.exit_code));
                slot.state = ProcessState::Free;
                slot.pid = 0;
                slot.parent_pid = 0;
                slot.exit_code = 0;
                return result;
            }
        }
    }
    None
}

/// Count of slots currently not `Free` (any combination of Running +
/// Zombie). Useful for tests + ps.
pub fn live_count() -> u32 {
    let mut total = 0u32;
    unsafe {
        let table = (&raw mut TABLE) as *mut Process;
        for i in 0..MAX_PROCESSES {
            let slot = &*table.add(i);
            if slot.state != ProcessState::Free {
                total += 1;
            }
        }
    }
    total
}

/// Copy the occupied slots out of the table.
///
/// Returns how many were written. Everything the shell's `ps` prints comes from one
/// call of this, so two rows it shows cannot come from two different moments.
pub fn snapshot(out: &mut [(u32, u32, u8, u64)]) -> usize {
    let mut n = 0;
    // SAFETY: same access as `live_count` -- a read of a table only the boot CPU
    // writes, taken while nothing else is spawning or reaping.
    unsafe {
        let table = (&raw mut TABLE) as *mut Process;
        for i in 0..MAX_PROCESSES {
            if n == out.len() {
                break;
            }
            let slot = &*table.add(i);
            let state = match slot.state {
                ProcessState::Free => continue,
                ProcessState::Running => b'R',
                ProcessState::Zombie => b'Z',
            };
            out[n] = (slot.pid, slot.parent_pid, state, slot.exit_code);
            n += 1;
        }
    }
    n
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    SpawnFailed,
    LiveCountWrong { expected: u32, got: u32 },
    ReapFailed,
    ExitCodeMismatch { expected: u64, got: u64 },
}

/// Boot self-check exercising
/// `spawn -> exit -> wait -> verify exit code` for three children.
/// Exit codes handed to the three children in [`self_check`].
const EXIT_CODES: [u64; 3] = [0, 1, 42];

pub fn self_check() -> Result<(), SelfCheckErr> {
    let parent: u32 = 1;
    for code in EXIT_CODES {
        spawn_zombie(parent, code).map_err(|_| SelfCheckErr::SpawnFailed)?;
    }

    if live_count() != 3 {
        return Err(SelfCheckErr::LiveCountWrong {
            expected: 3,
            got: live_count(),
        });
    }

    let mut sum: u64 = 0;
    for _ in 0..3 {
        let (_, code) = wait(parent).ok_or(SelfCheckErr::ReapFailed)?;
        sum = sum.saturating_add(code);
    }
    // The three exit codes the children above were given, added up. Written out so a
    // change to one of them is visible here rather than as a bare total.
    let expected_sum: u64 = EXIT_CODES.iter().sum();
    if sum != expected_sum {
        return Err(SelfCheckErr::ExitCodeMismatch {
            expected: expected_sum,
            got: sum,
        });
    }
    if live_count() != 0 {
        return Err(SelfCheckErr::LiveCountWrong {
            expected: 0,
            got: live_count(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reports_running_and_zombie_but_not_free_slots() {
        // SAFETY: a host test, single-threaded within this test binary's use of the
        // table; `init` is what the boot path calls for the same reason.
        unsafe { init() };
        let running = spawn(1).expect("spawn");
        let zombie = spawn_zombie(1, 7).expect("spawn_zombie");

        let mut rows = [(0u32, 0u32, 0u8, 0u64); MAX_PROCESSES];
        let n = snapshot(&mut rows);
        assert_eq!(n, 2, "two occupied slots, the rest are free");

        let r = rows[..n].iter().find(|r| r.0 == running).expect("running row");
        assert_eq!(r.2, b'R');
        let z = rows[..n].iter().find(|r| r.0 == zombie).expect("zombie row");
        assert_eq!(z.2, b'Z');
        assert_eq!(z.3, 7, "the zombie carries its exit code");
    }

    #[test]
    fn snapshot_stops_at_the_end_of_a_short_buffer() {
        // SAFETY: as above.
        unsafe { init() };
        for _ in 0..4 {
            spawn(1).expect("spawn");
        }
        let mut rows = [(0u32, 0u32, 0u8, 0u64); 2];
        assert_eq!(snapshot(&mut rows), 2, "writes no more than the buffer holds");
    }
}
