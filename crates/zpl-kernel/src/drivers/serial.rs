//! The boot log: where a marker goes after something decides to emit it.
//!
//! One line of kernel output can have several destinations -- the COM1 port, an
//! in-memory ring, a lane-tagged channel a test reads back -- and this module is
//! the plumbing that routes it, counts what it drops, and lets a test choose a
//! different arrangement without the callers knowing.
//!
//! Most of it exists to make the boot log testable on a host, where there is no
//! serial port to write to.
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Debug, Clone, Copy)]
pub struct BootLogEntry {
    pub seq: u32,
    pub lane: u8,
    pub source: u16,
    pub flags: u16,
    pub code: u16,
    pub value: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct BootLogFrameV1 {
    pub version: u16,
    pub lane: u8,
    pub source: u16,
    pub flags: u16,
    pub code: u16,
    pub seq: u32,
    pub value: u64,
}

impl BootLogEntry {
    pub const fn empty() -> Self {
        Self {
            seq: 0,
            lane: 0,
            source: 0,
            flags: 0,
            code: 0,
            value: 0,
        }
    }
}

impl BootLogFrameV1 {
    pub const VERSION: u16 = 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootLogSinkMode {
    BufferOnly,
    MirrorCom1,
    MirrorCom1AndVga,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootLogRingWriteMode {
    GlobalLock,
    LockFreeBestEffort,
    SourceShardedLockFree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootLogLanePreset {
    All,
    ContextOnly,
    ContextAndHealth,
    ContextAndHeartbeat,
}

#[derive(Debug, Clone, Copy)]
pub struct BootLogStats {
    pub ring_capacity: usize,
    pub ring_len: usize,
    pub ring_head: usize,
    pub frame_version: u16,
    pub sink_mode: BootLogSinkMode,
    pub ring_write_mode: BootLogRingWriteMode,
    pub dropped_sink_writes: u32,
    pub dropped_ring_writes: u32,
    pub overwritten_ring_entries: u32,
    pub sink_try_lock_spins: u32,
    pub ring_try_lock_spins: u32,
    pub sink_lane_mask: u32,
    pub source_channels: usize,
    pub source_channel_capacity: usize,
    pub source_channel_occupancy: [u32; BOOT_LOG_SOURCE_CHANNELS],
    pub source_online_count: u32,
    pub source_heartbeat_observed: u32,
    pub source_online_bitmap_words: usize,
    pub lane_counts: [u32; BOOT_LOG_MAX_LANES],
}

#[derive(Debug, Clone, Copy)]
pub struct BootSourceMetrics {
    pub source: u16,
    pub online: bool,
    pub heartbeat_count: u32,
    pub last_heartbeat_seq: u32,
}

pub const BOOT_LOG_CAPACITY: usize = 128;
pub const BOOT_LOG_SOURCE_CHANNELS: usize = 8;
pub const BOOT_LOG_SOURCE_CHANNEL_CAPACITY: usize = 32;
const BOOT_LOG_MAX_SNAPSHOT: usize = BOOT_LOG_SOURCE_CHANNELS * BOOT_LOG_SOURCE_CHANNEL_CAPACITY;

pub const LOG_BOOT_MAGIC: u16 = 1;
pub const LOG_BOOT_MEMORY_BYTES: u16 = 2;
pub const LOG_BOOT_CPU_COUNT: u16 = 3;
pub const LOG_BOOT_MEMORY_MAP_ENTRIES: u16 = 4;
pub const LOG_BOOT_RESERVED_BYTES: u16 = 5;
pub const LOG_BOOT_FREE_BYTES: u16 = 6;
pub const LOG_BOOT_FIRST_TICK: u16 = 7;
pub const LOG_BOOT_LOOP_HEARTBEAT: u16 = 8;
pub const LOG_BOOT_SINK_DROP_COUNT: u16 = 9;
pub const LOG_BOOT_RING_DROP_COUNT: u16 = 10;
pub const LOG_BOOT_RING_OVERWRITE_COUNT: u16 = 11;
pub const LOG_BOOT_SOURCE_ONLINE_COUNT: u16 = 12;
pub const LOG_BOOT_SOURCE_ONLINE_EVENT: u16 = 13;
pub const LOG_BOOT_SOURCE_OFFLINE_EVENT: u16 = 14;
pub const LOG_BOOT_SOURCE_HEARTBEAT: u16 = 15;
pub const BOOT_LOG_MAX_LANES: usize = 8;
pub const BOOT_LOG_LANE_CONTEXT: u8 = 0;
pub const BOOT_LOG_LANE_HEARTBEAT: u8 = 1;
pub const BOOT_LOG_LANE_SINK_HEALTH: u8 = 2;
pub const BOOT_LOG_FLAG_HEALTH_SAMPLE: u16 = 1 << 0;
pub const BOOT_LOG_FLAG_SOURCE_LIFECYCLE: u16 = 1 << 1;
pub const BOOT_LOG_FLAG_SOURCE_HEARTBEAT: u16 = 1 << 2;
pub const BOOT_LOG_MAX_SOURCE_IDS: usize = 256;
const BOOT_LOG_SOURCE_BITMAP_WORDS: usize = BOOT_LOG_MAX_SOURCE_IDS / 32;

static mut BOOT_LOG: [Option<BootLogEntry>; BOOT_LOG_CAPACITY] = [None; BOOT_LOG_CAPACITY];
static BOOT_LOG_HEAD: AtomicU32 = AtomicU32::new(0);
static BOOT_LOG_LEN: AtomicU32 = AtomicU32::new(0);
static mut BOOT_LOG_SOURCE_SHARDS: [[Option<BootLogEntry>; BOOT_LOG_SOURCE_CHANNEL_CAPACITY];
    BOOT_LOG_SOURCE_CHANNELS] = [[None; BOOT_LOG_SOURCE_CHANNEL_CAPACITY]; BOOT_LOG_SOURCE_CHANNELS];
static BOOT_LOG_SOURCE_HEADS: [AtomicU32; BOOT_LOG_SOURCE_CHANNELS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static BOOT_LOG_SOURCE_LENS: [AtomicU32; BOOT_LOG_SOURCE_CHANNELS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static mut BOOT_LOG_SINK_MODE: BootLogSinkMode = BootLogSinkMode::BufferOnly;
static mut BOOT_LOG_RING_WRITE_MODE: BootLogRingWriteMode = BootLogRingWriteMode::GlobalLock;
static BOOT_LOG_LOCK: AtomicBool = AtomicBool::new(false);
static SINK_IO_LOCK: AtomicBool = AtomicBool::new(false);
static DROPPED_SINK_WRITES: AtomicU32 = AtomicU32::new(0);
static DROPPED_RING_WRITES: AtomicU32 = AtomicU32::new(0);
static OVERWRITTEN_RING_ENTRIES: AtomicU32 = AtomicU32::new(0);
static NEXT_BOOT_LOG_SEQ: AtomicU32 = AtomicU32::new(1);
static CURRENT_BOOT_SOURCE_ID: AtomicU32 = AtomicU32::new(0);
#[cfg(any())]
static SOURCE_HEARTBEAT_COUNTS: [AtomicU32; BOOT_LOG_MAX_SOURCE_IDS] = [
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
];
#[cfg(any())]
static SOURCE_LAST_HEARTBEAT_SEQ: [AtomicU32; BOOT_LOG_MAX_SOURCE_IDS] = [
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
];
static SOURCE_HEARTBEAT_COUNTS: [AtomicU32; BOOT_LOG_MAX_SOURCE_IDS] =
    [const { AtomicU32::new(0) }; BOOT_LOG_MAX_SOURCE_IDS];
static SOURCE_LAST_HEARTBEAT_SEQ: [AtomicU32; BOOT_LOG_MAX_SOURCE_IDS] =
    [const { AtomicU32::new(0) }; BOOT_LOG_MAX_SOURCE_IDS];
static SOURCE_ONLINE_BITMAP: [AtomicU32; BOOT_LOG_SOURCE_BITMAP_WORDS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static LANE_COUNTS: [AtomicU32; BOOT_LOG_MAX_LANES] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
const DEFAULT_SINK_TRY_LOCK_SPINS: u32 = 1024;
static SINK_TRY_LOCK_SPINS: AtomicU32 = AtomicU32::new(DEFAULT_SINK_TRY_LOCK_SPINS);
const DEFAULT_RING_TRY_LOCK_SPINS: u32 = 4096;
static RING_TRY_LOCK_SPINS: AtomicU32 = AtomicU32::new(DEFAULT_RING_TRY_LOCK_SPINS);
const ALL_LANES_MASK: u32 = (1u32 << BOOT_LOG_MAX_LANES) - 1;
static SINK_LANE_MASK: AtomicU32 = AtomicU32::new(ALL_LANES_MASK);
const BOOT_CONTEXT_LANE_BIT: u32 = 1u32 << BOOT_LOG_LANE_CONTEXT;
const HEARTBEAT_LANE_BIT: u32 = 1u32 << BOOT_LOG_LANE_HEARTBEAT;
const SINK_HEALTH_LANE_BIT: u32 = 1u32 << BOOT_LOG_LANE_SINK_HEALTH;

pub fn set_boot_log_sink_mode(mode: BootLogSinkMode) {
    // Early boot scaffold is single-threaded for now.
    unsafe {
        BOOT_LOG_SINK_MODE = mode;
    }
}

pub fn boot_log_sink_mode() -> BootLogSinkMode {
    unsafe { BOOT_LOG_SINK_MODE }
}

pub fn reset_boot_log_sink_mode() {
    set_boot_log_sink_mode(BootLogSinkMode::BufferOnly);
}

pub fn set_boot_log_ring_write_mode(mode: BootLogRingWriteMode) {
    unsafe {
        BOOT_LOG_RING_WRITE_MODE = mode;
    }
}

pub fn boot_log_ring_write_mode() -> BootLogRingWriteMode {
    unsafe { BOOT_LOG_RING_WRITE_MODE }
}

pub fn reset_boot_log_ring_write_mode() {
    set_boot_log_ring_write_mode(BootLogRingWriteMode::GlobalLock);
}

pub fn log_boot_entry(code: u16, value: u64) {
    log_boot_entry_on_lane_from_source(
        BOOT_LOG_LANE_CONTEXT,
        current_boot_source_id(),
        0,
        code,
        value,
    );
}

pub fn log_boot_entry_for_source(source: u16, code: u16, value: u64) {
    log_boot_entry_on_lane_from_source(BOOT_LOG_LANE_CONTEXT, source, 0, code, value);
}

pub fn log_boot_entry_on_lane(lane: u8, code: u16, value: u64) {
    log_boot_entry_on_lane_from_source(lane, current_boot_source_id(), 0, code, value);
}

pub fn set_current_boot_source_id(source: u16) {
    CURRENT_BOOT_SOURCE_ID.store(source as u32, Ordering::Relaxed);
}

pub fn current_boot_source_id() -> u16 {
    (CURRENT_BOOT_SOURCE_ID.load(Ordering::Relaxed) & 0xffff) as u16
}

pub fn reset_current_boot_source_id() {
    CURRENT_BOOT_SOURCE_ID.store(0, Ordering::Relaxed);
}

pub fn set_boot_source_online(source: u16, online: bool) {
    let idx = normalize_source_id(source) as usize;
    let word = idx / 32;
    let bit = idx % 32;
    let mask = 1u32 << bit;
    if online {
        SOURCE_ONLINE_BITMAP[word].fetch_or(mask, Ordering::Relaxed);
    } else {
        SOURCE_ONLINE_BITMAP[word].fetch_and(!mask, Ordering::Relaxed);
    }
}

pub fn mark_current_boot_source_online() {
    let source = current_boot_source_id();
    mark_boot_source_online(source);
}

pub fn mark_current_boot_source_offline() {
    let source = current_boot_source_id();
    mark_boot_source_offline(source);
}

pub fn mark_boot_source_online(source: u16) {
    let source_norm = normalize_source_id(source);
    set_boot_source_online(source_norm, true);
    log_boot_entry_on_lane_from_source(
        BOOT_LOG_LANE_SINK_HEALTH,
        source_norm,
        BOOT_LOG_FLAG_SOURCE_LIFECYCLE,
        LOG_BOOT_SOURCE_ONLINE_EVENT,
        source_norm as u64,
    );
}

pub fn mark_boot_source_offline(source: u16) {
    let source_norm = normalize_source_id(source);
    set_boot_source_online(source_norm, false);
    log_boot_entry_on_lane_from_source(
        BOOT_LOG_LANE_SINK_HEALTH,
        source_norm,
        BOOT_LOG_FLAG_SOURCE_LIFECYCLE,
        LOG_BOOT_SOURCE_OFFLINE_EVENT,
        source_norm as u64,
    );
}

pub fn is_boot_source_online(source: u16) -> bool {
    let idx = normalize_source_id(source) as usize;
    let word = idx / 32;
    let bit = idx % 32;
    let mask = 1u32 << bit;
    (SOURCE_ONLINE_BITMAP[word].load(Ordering::Relaxed) & mask) != 0
}

pub fn boot_source_online_count() -> u32 {
    let mut total = 0u32;
    for word in &SOURCE_ONLINE_BITMAP {
        total = total.saturating_add(word.load(Ordering::Relaxed).count_ones());
    }
    total
}

pub fn reset_boot_source_online_bitmap() {
    for word in &SOURCE_ONLINE_BITMAP {
        word.store(0, Ordering::Relaxed);
    }
}

pub fn snapshot_boot_sources_online(out: &mut [u16]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut written = 0usize;
    for id in 0..BOOT_LOG_MAX_SOURCE_IDS {
        if !is_boot_source_online(id as u16) {
            continue;
        }
        out[written] = id as u16;
        written += 1;
        if written >= out.len() {
            break;
        }
    }
    written
}

pub fn boot_source_heartbeat_observed_count() -> u32 {
    let mut total = 0u32;
    for counter in SOURCE_HEARTBEAT_COUNTS.iter().take(BOOT_LOG_MAX_SOURCE_IDS) {
        if counter.load(Ordering::Relaxed) > 0 {
            total = total.saturating_add(1);
        }
    }
    total
}

pub fn reset_boot_source_heartbeat_metrics() {
    for idx in 0..BOOT_LOG_MAX_SOURCE_IDS {
        SOURCE_HEARTBEAT_COUNTS[idx].store(0, Ordering::Relaxed);
        SOURCE_LAST_HEARTBEAT_SEQ[idx].store(0, Ordering::Relaxed);
    }
}

pub fn record_boot_source_heartbeat(source: u16, heartbeat_value: u64) {
    let source_norm = normalize_source_id(source);
    let idx = source_norm as usize;
    let seq = log_boot_entry_on_lane_from_source_with_seq(
        BOOT_LOG_LANE_HEARTBEAT,
        source_norm,
        BOOT_LOG_FLAG_SOURCE_HEARTBEAT,
        LOG_BOOT_SOURCE_HEARTBEAT,
        heartbeat_value,
    );
    SOURCE_HEARTBEAT_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
    SOURCE_LAST_HEARTBEAT_SEQ[idx].store(seq, Ordering::Relaxed);
}

pub fn record_current_boot_source_heartbeat(heartbeat_value: u64) {
    record_boot_source_heartbeat(current_boot_source_id(), heartbeat_value);
}

pub fn snapshot_boot_source_metrics(out: &mut [BootSourceMetrics]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut written = 0usize;
    for id in 0..BOOT_LOG_MAX_SOURCE_IDS {
        let heartbeat_count = SOURCE_HEARTBEAT_COUNTS[id].load(Ordering::Relaxed);
        let online = is_boot_source_online(id as u16);
        if !online && heartbeat_count == 0 {
            continue;
        }
        out[written] = BootSourceMetrics {
            source: id as u16,
            online,
            heartbeat_count,
            last_heartbeat_seq: SOURCE_LAST_HEARTBEAT_SEQ[id].load(Ordering::Relaxed),
        };
        written += 1;
        if written >= out.len() {
            break;
        }
    }
    written
}

pub fn log_boot_entry_on_lane_from_source(
    lane: u8,
    source: u16,
    flags: u16,
    code: u16,
    value: u64,
) {
    let _ = log_boot_entry_on_lane_from_source_with_seq(lane, source, flags, code, value);
}

pub fn log_boot_text_line(text: &str) {
    #[cfg(all(target_os = "none", target_arch = "x86_64"))]
    {
        write_com1_text_line(text);
        return;
    }
    #[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
    {
        let _ = text;
    }
}

pub fn init_early_serial() {
    init_com1();
}

pub fn log_boot_entry_on_lane_from_source_with_seq(
    lane: u8,
    source: u16,
    flags: u16,
    code: u16,
    value: u64,
) -> u32 {
    let lane_norm = normalize_lane(lane);
    let lane_idx = lane_norm as usize;
    let seq = NEXT_BOOT_LOG_SEQ.fetch_add(1, Ordering::Relaxed);
    let entry = BootLogEntry {
        seq,
        lane: lane_norm,
        source,
        flags,
        code,
        value,
    };
    let ring_ok = match boot_log_ring_write_mode() {
        BootLogRingWriteMode::GlobalLock => write_ring_global_lock(entry),
        BootLogRingWriteMode::LockFreeBestEffort => write_ring_lock_free(entry),
        BootLogRingWriteMode::SourceShardedLockFree => write_ring_source_sharded(entry),
    };
    if !ring_ok {
        DROPPED_RING_WRITES.fetch_add(1, Ordering::Relaxed);
    } else {
        LANE_COUNTS[lane_idx].fetch_add(1, Ordering::Relaxed);
    }

    if !should_mirror_lane(lane_norm) {
        return seq;
    }
    if !try_lock(&SINK_IO_LOCK, sink_try_lock_spins()) {
        DROPPED_SINK_WRITES.fetch_add(1, Ordering::Relaxed);
        return seq;
    }
    match boot_log_sink_mode() {
        BootLogSinkMode::BufferOnly => {}
        BootLogSinkMode::MirrorCom1 => {
            write_com1_entry(entry);
        }
        BootLogSinkMode::MirrorCom1AndVga => {
            write_com1_entry(entry);
            write_vga_entry(entry);
        }
    }
    unlock(&SINK_IO_LOCK);
    seq
}

pub fn dropped_sink_writes() -> u32 {
    DROPPED_SINK_WRITES.load(Ordering::Relaxed)
}

pub fn reset_dropped_sink_writes() {
    DROPPED_SINK_WRITES.store(0, Ordering::Relaxed);
}

pub fn dropped_ring_writes() -> u32 {
    DROPPED_RING_WRITES.load(Ordering::Relaxed)
}

pub fn reset_dropped_ring_writes() {
    DROPPED_RING_WRITES.store(0, Ordering::Relaxed);
}

pub fn overwritten_ring_entries() -> u32 {
    OVERWRITTEN_RING_ENTRIES.load(Ordering::Relaxed)
}

pub fn reset_overwritten_ring_entries() {
    OVERWRITTEN_RING_ENTRIES.store(0, Ordering::Relaxed);
}

pub fn set_sink_lane_mask(mask: u32) {
    SINK_LANE_MASK.store(mask, Ordering::Relaxed);
}

pub fn sink_lane_mask() -> u32 {
    SINK_LANE_MASK.load(Ordering::Relaxed)
}

pub fn reset_sink_lane_mask() {
    set_sink_lane_mask(ALL_LANES_MASK);
}

pub fn apply_sink_lane_preset(preset: BootLogLanePreset) {
    let mask = match preset {
        BootLogLanePreset::All => ALL_LANES_MASK,
        BootLogLanePreset::ContextOnly => BOOT_CONTEXT_LANE_BIT,
        BootLogLanePreset::ContextAndHealth => BOOT_CONTEXT_LANE_BIT | SINK_HEALTH_LANE_BIT,
        BootLogLanePreset::ContextAndHeartbeat => BOOT_CONTEXT_LANE_BIT | HEARTBEAT_LANE_BIT,
    };
    set_sink_lane_mask(mask);
}

pub fn set_sink_lane_enabled(lane: u8, enabled: bool) {
    let shift = normalize_lane(lane) as u32;
    let lane_bit = 1u32 << shift;
    let mut mask = sink_lane_mask();
    if enabled {
        mask |= lane_bit;
    } else {
        mask &= !lane_bit;
    }
    set_sink_lane_mask(mask);
}

pub fn snapshot_lane_counts(out: &mut [u32]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let written = out.len().min(BOOT_LOG_MAX_LANES);
    for (idx, slot) in out.iter_mut().take(written).enumerate() {
        *slot = LANE_COUNTS[idx].load(Ordering::Relaxed);
    }
    written
}

pub fn set_sink_try_lock_spins(spins: u32) {
    if spins == 0 {
        SINK_TRY_LOCK_SPINS.store(DEFAULT_SINK_TRY_LOCK_SPINS, Ordering::Relaxed);
    } else {
        SINK_TRY_LOCK_SPINS.store(spins, Ordering::Relaxed);
    }
}

pub fn sink_try_lock_spins() -> u32 {
    SINK_TRY_LOCK_SPINS.load(Ordering::Relaxed)
}

pub fn reset_sink_try_lock_spins() {
    SINK_TRY_LOCK_SPINS.store(DEFAULT_SINK_TRY_LOCK_SPINS, Ordering::Relaxed);
}

pub fn set_ring_try_lock_spins(spins: u32) {
    if spins == 0 {
        RING_TRY_LOCK_SPINS.store(DEFAULT_RING_TRY_LOCK_SPINS, Ordering::Relaxed);
    } else {
        RING_TRY_LOCK_SPINS.store(spins, Ordering::Relaxed);
    }
}

pub fn ring_try_lock_spins() -> u32 {
    RING_TRY_LOCK_SPINS.load(Ordering::Relaxed)
}

pub fn reset_ring_try_lock_spins() {
    RING_TRY_LOCK_SPINS.store(DEFAULT_RING_TRY_LOCK_SPINS, Ordering::Relaxed);
}

pub fn reset_boot_log_buffer() {
    lock(&BOOT_LOG_LOCK);
    unsafe {
        BOOT_LOG = [None; BOOT_LOG_CAPACITY];
        BOOT_LOG_HEAD.store(0, Ordering::Relaxed);
        BOOT_LOG_LEN.store(0, Ordering::Relaxed);
        BOOT_LOG_SOURCE_SHARDS =
            [[None; BOOT_LOG_SOURCE_CHANNEL_CAPACITY]; BOOT_LOG_SOURCE_CHANNELS];
        for idx in 0..BOOT_LOG_SOURCE_CHANNELS {
            BOOT_LOG_SOURCE_HEADS[idx].store(0, Ordering::Relaxed);
            BOOT_LOG_SOURCE_LENS[idx].store(0, Ordering::Relaxed);
        }
    }
    unlock(&BOOT_LOG_LOCK);
}

pub fn reset_lane_counts() {
    for counter in &LANE_COUNTS {
        counter.store(0, Ordering::Relaxed);
    }
}

pub fn reset_boot_log_state() {
    reset_boot_log_buffer();
    reset_lane_counts();
    reset_dropped_sink_writes();
    reset_dropped_ring_writes();
    reset_overwritten_ring_entries();
    reset_boot_log_sink_mode();
    reset_boot_log_ring_write_mode();
    reset_sink_lane_mask();
    reset_sink_try_lock_spins();
    reset_ring_try_lock_spins();
    reset_current_boot_source_id();
    reset_boot_source_online_bitmap();
    reset_boot_source_heartbeat_metrics();
    NEXT_BOOT_LOG_SEQ.store(1, Ordering::Relaxed);
}

pub fn snapshot_boot_log_stats() -> BootLogStats {
    let ring_len = BOOT_LOG_LEN.load(Ordering::Relaxed) as usize;
    let ring_head = BOOT_LOG_HEAD.load(Ordering::Relaxed) as usize;
    let mut lane_counts = [0u32; BOOT_LOG_MAX_LANES];
    let _ = snapshot_lane_counts(&mut lane_counts);
    let mut source_channel_occupancy = [0u32; BOOT_LOG_SOURCE_CHANNELS];
    let _ = snapshot_source_channel_occupancy(&mut source_channel_occupancy);

    BootLogStats {
        ring_capacity: BOOT_LOG_CAPACITY,
        ring_len,
        ring_head,
        frame_version: BootLogFrameV1::VERSION,
        sink_mode: boot_log_sink_mode(),
        ring_write_mode: boot_log_ring_write_mode(),
        dropped_sink_writes: dropped_sink_writes(),
        dropped_ring_writes: dropped_ring_writes(),
        overwritten_ring_entries: overwritten_ring_entries(),
        sink_try_lock_spins: sink_try_lock_spins(),
        ring_try_lock_spins: ring_try_lock_spins(),
        sink_lane_mask: sink_lane_mask(),
        source_channels: BOOT_LOG_SOURCE_CHANNELS,
        source_channel_capacity: BOOT_LOG_SOURCE_CHANNEL_CAPACITY,
        source_channel_occupancy,
        source_online_count: boot_source_online_count(),
        source_heartbeat_observed: boot_source_heartbeat_observed_count(),
        source_online_bitmap_words: BOOT_LOG_SOURCE_BITMAP_WORDS,
        lane_counts,
    }
}

pub fn encode_boot_log_frame_v1(entry: BootLogEntry) -> BootLogFrameV1 {
    BootLogFrameV1 {
        version: BootLogFrameV1::VERSION,
        lane: entry.lane,
        source: entry.source,
        flags: entry.flags,
        code: entry.code,
        seq: entry.seq,
        value: entry.value,
    }
}

pub fn snapshot_boot_log_frames_v1(out: &mut [BootLogFrameV1]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut entries = [BootLogEntry::empty(); BOOT_LOG_MAX_SNAPSHOT];
    let available = snapshot_boot_log(&mut entries);
    if available == 0 {
        return 0;
    }
    let to_copy = available.min(out.len());
    let start = available.saturating_sub(to_copy);
    let mut written = 0usize;
    for entry in entries.iter().skip(start).take(to_copy) {
        out[written] = encode_boot_log_frame_v1(*entry);
        written += 1;
    }
    written
}

fn should_mirror_lane(lane: u8) -> bool {
    let shift = normalize_lane(lane) as u32;
    let lane_bit = 1u32 << shift;
    (sink_lane_mask() & lane_bit) != 0
}

pub fn snapshot_source_channel_occupancy(out: &mut [u32]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let written = out.len().min(BOOT_LOG_SOURCE_CHANNELS);
    for (idx, slot) in out.iter_mut().take(written).enumerate() {
        *slot = BOOT_LOG_SOURCE_LENS[idx].load(Ordering::Relaxed);
    }
    written
}

fn write_ring_global_lock(entry: BootLogEntry) -> bool {
    if !try_lock(&BOOT_LOG_LOCK, ring_try_lock_spins()) {
        return false;
    }
    let ok = unsafe {
        let head = BOOT_LOG_HEAD.load(Ordering::Relaxed) as usize % BOOT_LOG_CAPACITY;
        let prev_len = BOOT_LOG_LEN.load(Ordering::Relaxed) as usize;
        BOOT_LOG[head] = Some(entry);
        BOOT_LOG_HEAD.store(((head + 1) % BOOT_LOG_CAPACITY) as u32, Ordering::Relaxed);
        if prev_len < BOOT_LOG_CAPACITY {
            BOOT_LOG_LEN.store((prev_len + 1) as u32, Ordering::Relaxed);
        } else {
            OVERWRITTEN_RING_ENTRIES.fetch_add(1, Ordering::Relaxed);
        }
        true
    };
    unlock(&BOOT_LOG_LOCK);
    ok
}

fn write_ring_lock_free(entry: BootLogEntry) -> bool {
    let head = BOOT_LOG_HEAD.fetch_add(1, Ordering::Relaxed) as usize % BOOT_LOG_CAPACITY;
    unsafe {
        BOOT_LOG[head] = Some(entry);
    }

    loop {
        let prev = BOOT_LOG_LEN.load(Ordering::Relaxed) as usize;
        if prev >= BOOT_LOG_CAPACITY {
            OVERWRITTEN_RING_ENTRIES.fetch_add(1, Ordering::Relaxed);
            break;
        }
        if BOOT_LOG_LEN
            .compare_exchange(
                prev as u32,
                (prev + 1) as u32,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            break;
        }
    }
    true
}

fn write_ring_source_sharded(entry: BootLogEntry) -> bool {
    let shard = normalize_source(entry.source);
    let head = BOOT_LOG_SOURCE_HEADS[shard].fetch_add(1, Ordering::Relaxed) as usize
        % BOOT_LOG_SOURCE_CHANNEL_CAPACITY;
    unsafe {
        BOOT_LOG_SOURCE_SHARDS[shard][head] = Some(entry);
    }

    loop {
        let prev = BOOT_LOG_SOURCE_LENS[shard].load(Ordering::Relaxed) as usize;
        if prev >= BOOT_LOG_SOURCE_CHANNEL_CAPACITY {
            OVERWRITTEN_RING_ENTRIES.fetch_add(1, Ordering::Relaxed);
            break;
        }
        if BOOT_LOG_SOURCE_LENS[shard]
            .compare_exchange(
                prev as u32,
                (prev + 1) as u32,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            break;
        }
    }
    true
}

pub fn snapshot_boot_log(out: &mut [BootLogEntry]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if boot_log_ring_write_mode() == BootLogRingWriteMode::SourceShardedLockFree {
        return snapshot_boot_log_source_sharded(out);
    }
    lock(&BOOT_LOG_LOCK);
    unsafe {
        let ring_len = BOOT_LOG_LEN.load(Ordering::Relaxed) as usize;
        let ring_head = BOOT_LOG_HEAD.load(Ordering::Relaxed) as usize;
        if ring_len == 0 {
            unlock(&BOOT_LOG_LOCK);
            return 0;
        }
        let available = ring_len.min(BOOT_LOG_CAPACITY);
        let to_copy = available.min(out.len());
        let oldest = if ring_len < BOOT_LOG_CAPACITY {
            0
        } else {
            ring_head
        };
        let start = available.saturating_sub(to_copy);
        let mut written = 0usize;
        for i in 0..to_copy {
            let idx = (oldest + start + i) % BOOT_LOG_CAPACITY;
            if let Some(entry) = BOOT_LOG[idx] {
                out[written] = entry;
                written += 1;
            }
        }
        unlock(&BOOT_LOG_LOCK);
        written
    }
}

fn snapshot_boot_log_source_sharded(out: &mut [BootLogEntry]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut collected = [BootLogEntry::empty(); BOOT_LOG_MAX_SNAPSHOT];
    lock(&BOOT_LOG_LOCK);
    let mut collected_len = 0usize;
    unsafe {
        for shard in 0..BOOT_LOG_SOURCE_CHANNELS {
            let shard_len = BOOT_LOG_SOURCE_LENS[shard].load(Ordering::Relaxed) as usize;
            if shard_len == 0 {
                continue;
            }
            let available = shard_len.min(BOOT_LOG_SOURCE_CHANNEL_CAPACITY);
            let shard_head = BOOT_LOG_SOURCE_HEADS[shard].load(Ordering::Relaxed) as usize;
            let oldest = if shard_len < BOOT_LOG_SOURCE_CHANNEL_CAPACITY {
                0
            } else {
                shard_head % BOOT_LOG_SOURCE_CHANNEL_CAPACITY
            };
            for i in 0..available {
                if collected_len >= collected.len() {
                    break;
                }
                let idx = (oldest + i) % BOOT_LOG_SOURCE_CHANNEL_CAPACITY;
                if let Some(entry) = BOOT_LOG_SOURCE_SHARDS[shard][idx] {
                    collected[collected_len] = entry;
                    collected_len += 1;
                }
            }
        }
    }
    unlock(&BOOT_LOG_LOCK);

    if collected_len == 0 {
        return 0;
    }
    insertion_sort_entries_by_seq(&mut collected, collected_len);

    let to_copy = collected_len.min(out.len());
    let start = collected_len.saturating_sub(to_copy);
    let mut written = 0usize;
    for entry in collected.iter().skip(start).take(to_copy) {
        out[written] = *entry;
        written += 1;
    }
    written
}

pub fn snapshot_boot_log_for_lane(lane: u8, out: &mut [BootLogEntry]) -> usize {
    snapshot_boot_log_filtered(out, None, Some(normalize_lane(lane)), None)
}

pub fn snapshot_boot_log_for_code(code: u16, out: &mut [BootLogEntry]) -> usize {
    snapshot_boot_log_filtered(out, None, None, Some(code))
}

pub fn snapshot_boot_log_for_source(source: u16, out: &mut [BootLogEntry]) -> usize {
    snapshot_boot_log_filtered(out, Some(source), None, None)
}

pub fn snapshot_boot_log_for_lane_and_code(lane: u8, code: u16, out: &mut [BootLogEntry]) -> usize {
    snapshot_boot_log_filtered(out, None, Some(normalize_lane(lane)), Some(code))
}

pub fn snapshot_boot_log_for_source_and_code(
    source: u16,
    code: u16,
    out: &mut [BootLogEntry],
) -> usize {
    snapshot_boot_log_filtered(out, Some(source), None, Some(code))
}

pub fn snapshot_boot_log_for_source_and_lane(
    source: u16,
    lane: u8,
    out: &mut [BootLogEntry],
) -> usize {
    snapshot_boot_log_filtered(out, Some(source), Some(normalize_lane(lane)), None)
}

fn snapshot_boot_log_filtered(
    out: &mut [BootLogEntry],
    source_filter: Option<u16>,
    lane_filter: Option<u8>,
    code_filter: Option<u16>,
) -> usize {
    if out.is_empty() {
        return 0;
    }
    let mut all = [BootLogEntry::empty(); BOOT_LOG_MAX_SNAPSHOT];
    let available = snapshot_boot_log(&mut all);
    if available == 0 {
        return 0;
    }

    let mut matching = 0usize;
    for entry in all.iter().take(available) {
        if filter_matches(*entry, source_filter, lane_filter, code_filter) {
            matching += 1;
        }
    }
    if matching == 0 {
        return 0;
    }

    let to_copy = matching.min(out.len());
    let skip = matching.saturating_sub(to_copy);
    let mut seen = 0usize;
    let mut written = 0usize;
    for entry in all.iter().take(available) {
        if !filter_matches(*entry, source_filter, lane_filter, code_filter) {
            continue;
        }
        if seen < skip {
            seen += 1;
            continue;
        }
        out[written] = *entry;
        written += 1;
        if written >= to_copy {
            break;
        }
    }
    written
}

#[inline]
fn filter_matches(
    entry: BootLogEntry,
    source_filter: Option<u16>,
    lane_filter: Option<u8>,
    code_filter: Option<u16>,
) -> bool {
    if let Some(source) = source_filter {
        if entry.source != source {
            return false;
        }
    }
    if let Some(lane) = lane_filter {
        if entry.lane != lane {
            return false;
        }
    }
    if let Some(code) = code_filter {
        if entry.code != code {
            return false;
        }
    }
    true
}

#[inline]
fn normalize_lane(lane: u8) -> u8 {
    (lane as usize % BOOT_LOG_MAX_LANES) as u8
}

#[inline]
fn normalize_source(source: u16) -> usize {
    normalize_source_id(source) as usize % BOOT_LOG_SOURCE_CHANNELS
}

#[inline]
fn normalize_source_id(source: u16) -> u16 {
    (source as usize % BOOT_LOG_MAX_SOURCE_IDS) as u16
}

fn insertion_sort_entries_by_seq(entries: &mut [BootLogEntry; BOOT_LOG_MAX_SNAPSHOT], len: usize) {
    if len < 2 {
        return;
    }
    let mut i = 1usize;
    while i < len {
        let key = entries[i];
        let mut j = i;
        while j > 0 && entries[j - 1].seq > key.seq {
            entries[j] = entries[j - 1];
            j -= 1;
        }
        entries[j] = key;
        i += 1;
    }
}

fn lock(flag: &AtomicBool) {
    while flag
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn unlock(flag: &AtomicBool) {
    flag.store(false, Ordering::Release);
}

fn try_lock(flag: &AtomicBool, spins: u32) -> bool {
    let mut attempts = 0u32;
    while attempts < spins {
        if flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
        attempts = attempts.saturating_add(1);
        core::hint::spin_loop();
    }
    false
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_com1_entry(entry: BootLogEntry) {
    write_com1_byte(b'B');
    write_com1_byte(b'L');
    write_u8_hex(entry.lane);
    write_com1_byte(b'#');
    write_u32_hex(entry.seq);
    write_com1_byte(b'@');
    write_u16_hex(entry.source);
    write_com1_byte(b'/');
    write_u16_hex(entry.flags);
    write_u16_hex(entry.code);
    write_com1_byte(b'[');
    write_com1_label(entry.code);
    write_com1_byte(b']');
    write_com1_byte(b':');
    write_u64_hex(entry.value);
    write_com1_byte(b'\n');
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
fn write_com1_entry(_entry: BootLogEntry) {}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn init_com1() {
    // Disable interrupts.
    outb(0x3F9, 0x00);
    // Enable DLAB.
    outb(0x3FB, 0x80);
    // Set divisor to 3 (38400 baud).
    outb(0x3F8, 0x03);
    outb(0x3F9, 0x00);
    // 8 bits, no parity, one stop bit.
    outb(0x3FB, 0x03);
    // Enable FIFO, clear queues, 14-byte threshold.
    outb(0x3FA, 0xC7);
    // IRQs disabled, RTS/DSR set.
    outb(0x3FC, 0x03);
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
fn init_com1() {}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[allow(dead_code)]
fn write_com1_text_line(text: &str) {
    for byte in text.as_bytes() {
        write_com1_byte(*byte);
    }
    write_com1_byte(b'\n');
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
#[allow(dead_code)]
fn write_com1_text_line(_text: &str) {}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static mut VGA_ROW: usize = 0;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
static mut VGA_COL: usize = 0;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const VGA_BUFFER: *mut u8 = 0xb8000 as *mut u8;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const VGA_COLS: usize = 80;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const VGA_ROWS: usize = 25;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const VGA_CELL_BYTES: usize = 2;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const VGA_ATTR: u8 = 0x0f;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_entry(entry: BootLogEntry) {
    write_vga_byte(b'B');
    write_vga_byte(b'L');
    write_vga_u8_hex(entry.lane);
    write_vga_byte(b'#');
    write_vga_u32_hex(entry.seq);
    write_vga_byte(b'@');
    write_vga_u16_hex(entry.source);
    write_vga_byte(b'/');
    write_vga_u16_hex(entry.flags);
    write_vga_u16_hex(entry.code);
    write_vga_byte(b'[');
    write_vga_label(entry.code);
    write_vga_byte(b']');
    write_vga_byte(b':');
    write_vga_u64_hex(entry.value);
    write_vga_byte(b'\n');
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
fn write_vga_entry(_entry: BootLogEntry) {}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
#[allow(dead_code)]
fn write_vga_text_line(text: &str) {
    for byte in text.as_bytes() {
        write_vga_byte(*byte);
    }
    write_vga_byte(b'\n');
}

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
#[allow(dead_code)]
fn write_vga_text_line(_text: &str) {}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_u16_hex(v: u16) {
    for shift in [12u16, 8, 4, 0] {
        let nibble = ((v >> shift) & 0x0f) as u8;
        write_com1_byte(hex_nibble(nibble));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_u8_hex(v: u8) {
    for shift in [4u8, 0] {
        let nibble = (v >> shift) & 0x0f;
        write_com1_byte(hex_nibble(nibble));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_u64_hex(v: u64) {
    let mut shift = 60u8;
    loop {
        let nibble = ((v >> shift) & 0x0f) as u8;
        write_com1_byte(hex_nibble(nibble));
        if shift == 0 {
            break;
        }
        shift -= 4;
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_u32_hex(v: u32) {
    for shift in [28u32, 24, 20, 16, 12, 8, 4, 0] {
        let nibble = ((v >> shift) & 0x0f) as u8;
        write_com1_byte(hex_nibble(nibble));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_u16_hex(v: u16) {
    for shift in [12u16, 8, 4, 0] {
        let nibble = ((v >> shift) & 0x0f) as u8;
        write_vga_byte(hex_nibble(nibble));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_u8_hex(v: u8) {
    for shift in [4u8, 0] {
        let nibble = (v >> shift) & 0x0f;
        write_vga_byte(hex_nibble(nibble));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_u64_hex(v: u64) {
    let mut shift = 60u8;
    loop {
        let nibble = ((v >> shift) & 0x0f) as u8;
        write_vga_byte(hex_nibble(nibble));
        if shift == 0 {
            break;
        }
        shift -= 4;
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_u32_hex(v: u32) {
    for shift in [28u32, 24, 20, 16, 12, 8, 4, 0] {
        let nibble = ((v >> shift) & 0x0f) as u8;
        write_vga_byte(hex_nibble(nibble));
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn hex_nibble(v: u8) -> u8 {
    match v {
        0..=9 => b'0' + v,
        _ => b'a' + (v - 10),
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_com1_byte(value: u8) {
    wait_for_com1_tx_ready();
    // COM1 data port.
    outb(0x3F8, value);
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_com1_label(code: u16) {
    match code {
        LOG_BOOT_MAGIC => write_com1_bytes(b"MAGC"),
        LOG_BOOT_MEMORY_BYTES => write_com1_bytes(b"MEMB"),
        LOG_BOOT_CPU_COUNT => write_com1_bytes(b"CPUS"),
        LOG_BOOT_MEMORY_MAP_ENTRIES => write_com1_bytes(b"MMAP"),
        LOG_BOOT_RESERVED_BYTES => write_com1_bytes(b"RSVD"),
        LOG_BOOT_FREE_BYTES => write_com1_bytes(b"FREE"),
        LOG_BOOT_FIRST_TICK => write_com1_bytes(b"TIK0"),
        LOG_BOOT_LOOP_HEARTBEAT => write_com1_bytes(b"TIKH"),
        LOG_BOOT_SINK_DROP_COUNT => write_com1_bytes(b"DROP"),
        LOG_BOOT_RING_DROP_COUNT => write_com1_bytes(b"RDRP"),
        LOG_BOOT_RING_OVERWRITE_COUNT => write_com1_bytes(b"ROVR"),
        LOG_BOOT_SOURCE_ONLINE_COUNT => write_com1_bytes(b"SRCS"),
        LOG_BOOT_SOURCE_ONLINE_EVENT => write_com1_bytes(b"SONL"),
        LOG_BOOT_SOURCE_OFFLINE_EVENT => write_com1_bytes(b"SOFF"),
        LOG_BOOT_SOURCE_HEARTBEAT => write_com1_bytes(b"SHBT"),
        _ => write_com1_bytes(b"UNKN"),
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_com1_bytes(bytes: &[u8]) {
    for b in bytes {
        write_com1_byte(*b);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_byte(value: u8) {
    unsafe {
        match value {
            b'\n' => {
                vga_advance_line();
                return;
            }
            b'\r' => {
                VGA_COL = 0;
                return;
            }
            _ => {}
        }

        if VGA_COL >= VGA_COLS {
            vga_advance_line();
        }

        let cell = VGA_ROW * VGA_COLS + VGA_COL;
        let offset = cell * VGA_CELL_BYTES;
        core::ptr::write_volatile(VGA_BUFFER.add(offset), value);
        core::ptr::write_volatile(VGA_BUFFER.add(offset + 1), VGA_ATTR);
        VGA_COL += 1;
        if VGA_COL >= VGA_COLS {
            vga_advance_line();
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_label(code: u16) {
    match code {
        LOG_BOOT_MAGIC => write_vga_bytes(b"MAGC"),
        LOG_BOOT_MEMORY_BYTES => write_vga_bytes(b"MEMB"),
        LOG_BOOT_CPU_COUNT => write_vga_bytes(b"CPUS"),
        LOG_BOOT_MEMORY_MAP_ENTRIES => write_vga_bytes(b"MMAP"),
        LOG_BOOT_RESERVED_BYTES => write_vga_bytes(b"RSVD"),
        LOG_BOOT_FREE_BYTES => write_vga_bytes(b"FREE"),
        LOG_BOOT_FIRST_TICK => write_vga_bytes(b"TIK0"),
        LOG_BOOT_LOOP_HEARTBEAT => write_vga_bytes(b"TIKH"),
        LOG_BOOT_SINK_DROP_COUNT => write_vga_bytes(b"DROP"),
        LOG_BOOT_RING_DROP_COUNT => write_vga_bytes(b"RDRP"),
        LOG_BOOT_RING_OVERWRITE_COUNT => write_vga_bytes(b"ROVR"),
        LOG_BOOT_SOURCE_ONLINE_COUNT => write_vga_bytes(b"SRCS"),
        LOG_BOOT_SOURCE_ONLINE_EVENT => write_vga_bytes(b"SONL"),
        LOG_BOOT_SOURCE_OFFLINE_EVENT => write_vga_bytes(b"SOFF"),
        LOG_BOOT_SOURCE_HEARTBEAT => write_vga_bytes(b"SHBT"),
        _ => write_vga_bytes(b"UNKN"),
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn write_vga_bytes(bytes: &[u8]) {
    for b in bytes {
        write_vga_byte(*b);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn vga_advance_line() {
    unsafe {
        VGA_COL = 0;
        VGA_ROW += 1;
        if VGA_ROW >= VGA_ROWS {
            vga_scroll_up_one_row();
            VGA_ROW = VGA_ROWS - 1;
        }
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn vga_scroll_up_one_row() {
    unsafe {
        for row in 1..VGA_ROWS {
            for col in 0..VGA_COLS {
                let from_cell = row * VGA_COLS + col;
                let to_cell = (row - 1) * VGA_COLS + col;
                let from_offset = from_cell * VGA_CELL_BYTES;
                let to_offset = to_cell * VGA_CELL_BYTES;
                let ch = core::ptr::read_volatile(VGA_BUFFER.add(from_offset));
                let attr = core::ptr::read_volatile(VGA_BUFFER.add(from_offset + 1));
                core::ptr::write_volatile(VGA_BUFFER.add(to_offset), ch);
                core::ptr::write_volatile(VGA_BUFFER.add(to_offset + 1), attr);
            }
        }
        vga_clear_row(VGA_ROWS - 1);
    }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn vga_clear_row(row: usize) {
    unsafe {
        for col in 0..VGA_COLS {
            let cell = row * VGA_COLS + col;
            let offset = cell * VGA_CELL_BYTES;
            core::ptr::write_volatile(VGA_BUFFER.add(offset), b' ');
            core::ptr::write_volatile(VGA_BUFFER.add(offset + 1), VGA_ATTR);
        }
    }
}

/// COM1 only, and that is what makes the wrapper safe to call.
///
/// The three obligations in `arch::port` are met here for every call in this module:
/// the ports are the UART's, this module is the only thing in the kernel that drives
/// them, and the side effects -- sending a byte, arming the FIFO -- are the point.
/// Callers below therefore do not write `unsafe`, and do not each have to argue it.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn outb(port: u16, value: u8) {
    // SAFETY: COM1 is owned by this module; see the note above.
    unsafe { crate::arch::port::outb(port, value) }
}

/// One byte from COM1 if the line has one waiting, `None` otherwise.
///
/// Never blocks. A kernel that blocks on a serial port nobody is typing at is
/// a kernel that has stopped, and every automated run here feeds COM1 from a
/// file with no input side at all.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "net_console", feature = "shell")
))]
pub fn read_com1_byte() -> Option<u8> {
    // Line Status Register, bit 0: a byte is waiting in the receive register.
    const LSR: u16 = 0x3FD;
    const RBR: u16 = 0x3F8;
    if !com1_present() {
        return None;
    }
    if !lsr_says_byte_waiting(inb(LSR)) {
        return None;
    }
    Some(inb(RBR))
}

/// Does this Line Status Register byte mean a byte is waiting?
///
/// `lsr & 1` alone is not the answer. An x86 I/O port with nothing behind it reads back
/// `0xFF`, and `0xFF` has bit 0 set, so the naive test says "yes" forever on a machine
/// with no serial port. `0xFF` also claims every error bit and both empty-register bits
/// at once, which no working UART reports together, so it is read as "nobody home"
/// rather than as data.
///
/// Pure, and separate from the port read, so the rule can be tested on the host: the
/// machine where this went wrong is not one the test suite can boot.
// Dead only in a build without the readers that call it -- the rule still has to be
// testable on the host, which is where the machine that broke cannot be booted.
#[cfg_attr(not(any(feature = "shell", feature = "net_console")), allow(dead_code))]
pub(crate) const fn lsr_says_byte_waiting(lsr: u8) -> bool {
    lsr != 0xFF && (lsr & 1) != 0
}

/// Did the scratch register give back what was written to it?
///
/// The scratch register is the one UART register with no hardware meaning, so this is
/// the standard presence test. Pure for the same reason as above.
// Dead only in a build without the readers that call it -- the rule still has to be
// testable on the host, which is where the machine that broke cannot be booted.
#[cfg_attr(not(any(feature = "shell", feature = "net_console")), allow(dead_code))]
pub(crate) const fn scratch_echo_means_present(echoed: u8, written: u8) -> bool {
    echoed == written
}

/// Cached answer to "is there a UART at COM1 at all?". `0` unknown, `1` yes, `2` no.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "net_console", feature = "shell")
))]
static COM1_PRESENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Whether a serial port answers at COM1, probed once and remembered.
///
/// This exists because of a defect found on real hardware and invisible under QEMU. An
/// x86 I/O port with nothing behind it reads back `0xFF`, and `0xFF` has the Line Status
/// Register's "data ready" bit set. So on a laptop with no serial port, a reader that
/// trusts the LSR is told a byte is waiting, every single time, and hands back `0xFF`
/// forever. The shell asks COM1 before the keyboard, so it spent every poll swallowing
/// bytes from a port that does not exist and never reached the keyboard: the prompt
/// appeared and could not be typed at. Under QEMU, where COM1 always exists and the LSR
/// is honest, none of this shows.
///
/// The probe is the standard one: the scratch register at `+7` is the one UART register
/// with no hardware meaning, so a byte written there comes back unchanged if, and only
/// if, something is listening. An absent port returns `0xFF` and fails the comparison.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "net_console", feature = "shell")
))]
pub fn com1_present() -> bool {
    use core::sync::atomic::Ordering;

    const SCRATCH: u16 = 0x3FF;
    const PROBE: u8 = 0xA5;

    match COM1_PRESENT.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let saved = inb(SCRATCH);
    outb(SCRATCH, PROBE);
    let echoed = inb(SCRATCH);
    outb(SCRATCH, saved);
    let present = scratch_echo_means_present(echoed, PROBE);
    COM1_PRESENT.store(if present { 1 } else { 2 }, Ordering::Relaxed);
    present
}

#[cfg(not(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "net_console", feature = "shell")
)))]
pub fn com1_present() -> bool {
    false
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn inb(port: u16) -> u8 {
    // SAFETY: as `outb` above -- COM1, owned by this module.
    unsafe { crate::arch::port::inb(port) }
}

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn wait_for_com1_tx_ready() {
    // LSR bit 5 means transmitter holding register is empty.
    while (inb(0x3FD) & 0x20) == 0 {
        core::hint::spin_loop();
    }
}


#[cfg(test)]
mod com1_presence_tests {
    use super::{lsr_says_byte_waiting, scratch_echo_means_present};

    /// The defect, written down as a test.
    ///
    /// On a laptop with no serial port, reading the Line Status Register returns 0xFF,
    /// and the old rule -- bit 0 set means a byte is waiting -- said yes to that. The
    /// shell asked COM1 before the keyboard, so it swallowed 0xFF forever and the prompt
    /// could not be typed at. Under QEMU the port always exists, so nothing showed.
    #[test]
    fn an_absent_port_reading_all_ones_is_not_a_waiting_byte() {
        assert!(
            !lsr_says_byte_waiting(0xFF),
            "0xFF is an I/O port with nothing behind it, not a byte of input"
        );
    }

    #[test]
    fn a_real_status_byte_still_reports_its_data() {
        assert!(lsr_says_byte_waiting(0x01), "data-ready alone means a byte is waiting");
        assert!(lsr_says_byte_waiting(0x61), "data ready plus the two empty bits");
        assert!(!lsr_says_byte_waiting(0x00), "nothing set means nothing waiting");
        assert!(!lsr_says_byte_waiting(0x60), "both empty bits, no data-ready");
    }

    #[test]
    fn the_scratch_probe_rejects_an_absent_port_and_accepts_a_real_one() {
        assert!(!scratch_echo_means_present(0xFF, 0xA5), "an absent port reads all ones");
        assert!(!scratch_echo_means_present(0x00, 0xA5), "and a dead one reads zero");
        assert!(scratch_echo_means_present(0xA5, 0xA5), "a UART gives the byte back");
    }
}
