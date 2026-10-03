//! The kernel's own bookkeeping: tasks, the queue they wait in, and the state
//! that is carried from one tick to the next.
//!
//! Deliberately free of any `#[cfg(target_os = "none")]`, so all of it runs and
//! is tested on a host. The bare-metal half lives in `boot`, `interrupts` and
//! `scheduler`.
use crate::sched::policy_hook::KernelDecision;
use crate::audit::trace::TraceRing;

#[derive(Debug, Clone, Copy)]
pub struct Task {
    pub id: u64,
    pub priority: u8,
    pub bias_hint: f32,
}

pub const DEFAULT_QUEUE_CAPACITY: usize = 128;
pub const DEFAULT_TRACE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct TaskQueue<const N: usize> {
    pub data: [Option<Task>; N],
    pub head: usize,
    pub tail: usize,
    pub len: usize,
}

impl<const N: usize> TaskQueue<N> {
    pub const fn new() -> Self {
        Self {
            data: [None; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len >= N
    }

    pub fn enqueue(&mut self, task: Task) -> bool {
        if self.is_full() {
            return false;
        }
        self.data[self.tail] = Some(task);
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        true
    }

    pub fn dequeue(&mut self) -> Option<Task> {
        if self.is_empty() {
            return None;
        }
        let item = self.data[self.head];
        self.data[self.head] = None;
        self.head = (self.head + 1) % N;
        self.len -= 1;
        item
    }

    pub fn peek(&self) -> Option<Task> {
        self.data[self.head]
    }

    pub fn reprioritize(&mut self, task_id: u64, new_priority: u8) -> bool {
        for slot in &mut self.data {
            if let Some(task) = slot.as_mut() {
                if task.id == task_id {
                    task.priority = new_priority;
                    return true;
                }
            }
        }
        false
    }
}

impl<const N: usize> Default for TaskQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub type KernelTaskQueue = TaskQueue<DEFAULT_QUEUE_CAPACITY>;
pub type KernelTraceRing = TraceRing<DEFAULT_TRACE_CAPACITY>;

#[derive(Debug, Clone, Copy)]
pub struct KernelState {
    pub ticks: u64,
    pub queue: KernelTaskQueue,
    pub trace: KernelTraceRing,
    pub last_decision: KernelDecision,
}

impl KernelState {
    pub const fn new() -> Self {
        Self {
            ticks: 0,
            queue: KernelTaskQueue::new(),
            trace: KernelTraceRing::new(),
            last_decision: KernelDecision::Allow,
        }
    }

    pub fn on_tick(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
    }

    pub fn on_decision(&mut self, decision: KernelDecision) {
        self.last_decision = decision;
    }
}

impl Default for KernelState {
    fn default() -> Self {
        Self::new()
    }
}
