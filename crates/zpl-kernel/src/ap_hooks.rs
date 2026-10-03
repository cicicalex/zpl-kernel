//! Where events about secondary CPUs come from, and the queue they arrive in.
//!
//! An event source is a trait so the lifecycle bookkeeping can be driven by a
//! test, by a virtual machine, or eventually by real hardware, without any of
//! them knowing about the others. Today only the first two exist:
//! `NullApEventSource` produces nothing and `VmApEventSource` produces what a
//! test puts in it.
use crate::ap_lifecycle::{MAX_SECONDARY_BOOT_SOURCES, MAX_SOURCE_IDS};

pub const DEFAULT_AP_EVENT_QUEUE_CAPACITY: usize = 64;
const DEFAULT_VM_HEARTBEAT_INTERVAL_TICKS: u64 = 1024;
const DEFAULT_VM_ROTATION_INTERVAL_TICKS: u64 = 16384;

pub trait ApEventSource {
    fn poll_event(&mut self) -> Option<ApLifecycleEvent>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NullApEventSource;

impl ApEventSource for NullApEventSource {
    fn poll_event(&mut self) -> Option<ApLifecycleEvent> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VmApEventSource<const N: usize> {
    queue: ApEventQueue<N>,
    primary_source: u16,
    secondary_sources: [u16; MAX_SECONDARY_BOOT_SOURCES],
    secondary_count: usize,
    secondary_online: [bool; MAX_SECONDARY_BOOT_SOURCES],
    rotation_cursor: usize,
    bootstrapped: bool,
    last_heartbeat_tick: u64,
    last_rotation_tick: u64,
}

impl<const N: usize> VmApEventSource<N> {
    pub fn from_boot(primary_source: u16, cpu_count: u32) -> Self {
        let primary = primary_source % MAX_SOURCE_IDS;
        let mut secondary_sources = [0u16; MAX_SECONDARY_BOOT_SOURCES];
        let secondary_count = (cpu_count.saturating_sub(1) as usize).min(MAX_SECONDARY_BOOT_SOURCES);
        let mut idx = 0usize;
        while idx < secondary_count {
            secondary_sources[idx] = primary.wrapping_add((idx + 1) as u16) % MAX_SOURCE_IDS;
            idx += 1;
        }
        Self {
            queue: ApEventQueue::new(),
            primary_source: primary,
            secondary_sources,
            secondary_count,
            secondary_online: [false; MAX_SECONDARY_BOOT_SOURCES],
            rotation_cursor: 0,
            bootstrapped: false,
            last_heartbeat_tick: 0,
            last_rotation_tick: 0,
        }
    }

    pub fn seed_bootstrap_events(&mut self) {
        if self.bootstrapped {
            return;
        }
        self.queue.push(ApLifecycleEvent::Online {
            source: self.primary_source,
        });
        let mut idx = 0usize;
        while idx < self.secondary_count {
            self.secondary_online[idx] = true;
            self.queue.push(ApLifecycleEvent::Online {
                source: self.secondary_sources[idx],
            });
            idx += 1;
        }
        self.bootstrapped = true;
    }

    pub fn on_tick(&mut self, tick: u64) {
        if !self.bootstrapped {
            self.seed_bootstrap_events();
        }
        if tick.saturating_sub(self.last_heartbeat_tick) >= DEFAULT_VM_HEARTBEAT_INTERVAL_TICKS {
            self.last_heartbeat_tick = tick;
            self.queue.push(ApLifecycleEvent::Heartbeat {
                source: self.primary_source,
                tick,
            });
            let mut idx = 0usize;
            while idx < self.secondary_count {
                if self.secondary_online[idx] {
                    self.queue.push(ApLifecycleEvent::Heartbeat {
                        source: self.secondary_sources[idx],
                        tick,
                    });
                }
                idx += 1;
            }
        }
        if self.secondary_count == 0 {
            return;
        }
        if tick.saturating_sub(self.last_rotation_tick) >= DEFAULT_VM_ROTATION_INTERVAL_TICKS {
            self.last_rotation_tick = tick;
            let idx = self.rotation_cursor % self.secondary_count;
            let source = self.secondary_sources[idx];
            let now_online = !self.secondary_online[idx];
            self.secondary_online[idx] = now_online;
            if now_online {
                self.queue.push(ApLifecycleEvent::Online { source });
            } else {
                self.queue.push(ApLifecycleEvent::Offline { source });
            }
            self.rotation_cursor = (self.rotation_cursor + 1) % self.secondary_count;
        }
    }
}

impl<const N: usize> ApEventSource for VmApEventSource<N> {
    fn poll_event(&mut self) -> Option<ApLifecycleEvent> {
        self.queue.pop()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ApLifecycleEvent {
    Online { source: u16 },
    Offline { source: u16 },
    Heartbeat { source: u16, tick: u64 },
}

impl ApLifecycleEvent {
    pub fn normalize(self) -> Self {
        match self {
            Self::Online { source } => Self::Online {
                source: source % MAX_SOURCE_IDS,
            },
            Self::Offline { source } => Self::Offline {
                source: source % MAX_SOURCE_IDS,
            },
            Self::Heartbeat { source, tick } => Self::Heartbeat {
                source: source % MAX_SOURCE_IDS,
                tick,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ApEventQueue<const N: usize> {
    slots: [Option<ApLifecycleEvent>; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<const N: usize> ApEventQueue<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len >= N
    }

    pub fn push(&mut self, event: ApLifecycleEvent) -> bool {
        if self.is_full() || N == 0 {
            return false;
        }
        self.slots[self.tail] = Some(event.normalize());
        self.tail = (self.tail + 1) % N;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<ApLifecycleEvent> {
        if self.is_empty() || N == 0 {
            return None;
        }
        let event = self.slots[self.head];
        self.slots[self.head] = None;
        self.head = (self.head + 1) % N;
        self.len -= 1;
        event
    }
}

impl<const N: usize> Default for ApEventQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ApHookAdapter<const N: usize> {
    queue: ApEventQueue<N>,
    dropped_events: u32,
}

impl<const N: usize> ApHookAdapter<N> {
    pub const fn new() -> Self {
        Self {
            queue: ApEventQueue::new(),
            dropped_events: 0,
        }
    }

    pub fn register_online(&mut self, source: u16) {
        self.push_or_drop(ApLifecycleEvent::Online { source });
    }

    pub fn register_offline(&mut self, source: u16) {
        self.push_or_drop(ApLifecycleEvent::Offline { source });
    }

    pub fn register_heartbeat(&mut self, source: u16, tick: u64) {
        self.push_or_drop(ApLifecycleEvent::Heartbeat { source, tick });
    }

    pub fn poll_event(&mut self) -> Option<ApLifecycleEvent> {
        self.queue.pop()
    }

    pub fn ingest_from_source<S: ApEventSource>(&mut self, source: &mut S, max_events: usize) -> usize {
        if max_events == 0 {
            return 0;
        }
        let mut ingested = 0usize;
        while ingested < max_events {
            let Some(event) = source.poll_event() else {
                break;
            };
            self.push_or_drop(event);
            ingested += 1;
        }
        ingested
    }

    pub const fn pending_events(&self) -> usize {
        self.queue.len()
    }

    pub const fn dropped_events(&self) -> u32 {
        self.dropped_events
    }

    fn push_or_drop(&mut self, event: ApLifecycleEvent) {
        if !self.queue.push(event) {
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
    }
}

impl<const N: usize> Default for ApHookAdapter<N> {
    fn default() -> Self {
        Self::new()
    }
}
