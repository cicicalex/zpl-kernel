//! Hardware abstraction layer.
//!
//! Two traits decouple the kernel from concrete device drivers:
//!
//! - `BlockDevice` — fixed-size sector reads/writes.
//! - `NetDevice` — frame send/receive plus MAC address discovery.
//!
//! The kernel ships a `MockBlockDevice` + `MockNetDevice` for unit-style
//! self-checks; future virtio-blk (#15) and virtio-net (#20) drivers will
//! implement the same traits. PCI / NVMe back-ends slot in the same way.
//!
//! v0.1 design choices:
//! - No allocator dependency: I/O buffers are byte slices the caller
//!   owns. Block size is `512` bytes (industry standard).
//! - The mock devices keep their state in the device struct itself, not
//!   in a global. This means each test/instance can be private and the
//!   trait dispatch is exercised through fully owned objects.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

pub const BLOCK_SIZE: usize = 512;
pub const FRAME_MTU: usize = 1500;
pub const MAX_MOCK_BLOCKS: usize = 4;
pub const MAX_MOCK_FRAMES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalError {
    OutOfRange,
    Empty,
    BufferTooSmall,
}

/// A block-oriented storage device. Sector numbering starts at 0; sector
/// length is fixed at `BLOCK_SIZE` bytes.
pub trait BlockDevice {
    fn block_size(&self) -> usize;
    fn block_count(&self) -> u64;
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), HalError>;
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), HalError>;
}

/// A frame-oriented network device. Caller provides their own buffers
/// for both directions.
pub trait NetDevice {
    fn mac_addr(&self) -> [u8; 6];
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), HalError>;
    fn recv_frame(&mut self, frame: &mut [u8]) -> Result<usize, HalError>;
}

/// In-memory block device for self-checks.
pub struct MockBlockDevice {
    storage: [[u8; BLOCK_SIZE]; MAX_MOCK_BLOCKS],
}

impl MockBlockDevice {
    pub const fn new() -> Self {
        Self {
            storage: [[0; BLOCK_SIZE]; MAX_MOCK_BLOCKS],
        }
    }
}

impl BlockDevice for MockBlockDevice {
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn block_count(&self) -> u64 {
        MAX_MOCK_BLOCKS as u64
    }

    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), HalError> {
        if lba as usize >= MAX_MOCK_BLOCKS {
            return Err(HalError::OutOfRange);
        }
        if buf.len() < BLOCK_SIZE {
            return Err(HalError::BufferTooSmall);
        }
        buf[..BLOCK_SIZE].copy_from_slice(&self.storage[lba as usize]);
        Ok(())
    }

    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), HalError> {
        if lba as usize >= MAX_MOCK_BLOCKS {
            return Err(HalError::OutOfRange);
        }
        if buf.len() < BLOCK_SIZE {
            return Err(HalError::BufferTooSmall);
        }
        self.storage[lba as usize].copy_from_slice(&buf[..BLOCK_SIZE]);
        Ok(())
    }
}

/// In-memory network device for self-checks. RX is a tiny ring buffer of
/// pending frames seeded by the test harness; TX captures the most recent
/// outbound frame for inspection.
pub struct MockNetDevice {
    mac: [u8; 6],
    pending_rx: [Option<MockFrame>; MAX_MOCK_FRAMES],
    last_tx: Option<MockFrame>,
}

#[derive(Clone, Copy)]
pub struct MockFrame {
    pub bytes: [u8; FRAME_MTU],
    pub len: u16,
}

impl MockFrame {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; FRAME_MTU],
            len: 0,
        }
    }

    pub fn from_slice(data: &[u8]) -> Self {
        let mut frame = Self::empty();
        let n = core::cmp::min(data.len(), FRAME_MTU);
        frame.bytes[..n].copy_from_slice(&data[..n]);
        frame.len = n as u16;
        frame
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

impl MockNetDevice {
    pub const fn new(mac: [u8; 6]) -> Self {
        Self {
            mac,
            pending_rx: [const { None }; MAX_MOCK_FRAMES],
            last_tx: None,
        }
    }

    /// Inject a frame into the RX ring so a subsequent `recv_frame` can
    /// observe it.
    pub fn inject_rx(&mut self, frame: &[u8]) -> Result<(), HalError> {
        for slot in self.pending_rx.iter_mut() {
            if slot.is_none() {
                *slot = Some(MockFrame::from_slice(frame));
                return Ok(());
            }
        }
        Err(HalError::OutOfRange)
    }

    /// Inspect the last frame the kernel sent.
    pub fn last_tx_payload(&self) -> Option<&[u8]> {
        self.last_tx.as_ref().map(|f| f.payload())
    }
}

impl NetDevice for MockNetDevice {
    fn mac_addr(&self) -> [u8; 6] {
        self.mac
    }

    fn send_frame(&mut self, frame: &[u8]) -> Result<(), HalError> {
        if frame.len() > FRAME_MTU {
            return Err(HalError::BufferTooSmall);
        }
        self.last_tx = Some(MockFrame::from_slice(frame));
        Ok(())
    }

    fn recv_frame(&mut self, out: &mut [u8]) -> Result<usize, HalError> {
        for slot in self.pending_rx.iter_mut() {
            if let Some(frame) = slot.take() {
                let n = core::cmp::min(out.len(), frame.len as usize);
                out[..n].copy_from_slice(&frame.bytes[..n]);
                return Ok(n);
            }
        }
        Err(HalError::Empty)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    BlockReadbackMismatch,
    NetTxMismatch,
    NetRxMismatch,
    NetMacMismatch,
}

/// Storage for the self-check's two mock devices.
///
/// They are static, not locals, and that is the whole point. `MockNetDevice` holds
/// eight frames of 1500 bytes, so it is about thirteen kilobytes; the block mock
/// adds more; and the kernel stack this runs on is eight. As locals they wrote
/// several kilobytes past the end of the stack, into whatever statics happened to
/// follow.
///
/// That had been true for a long time and did no visible harm, because what it
/// landed on did not matter. It started mattering when an unrelated one-byte static
/// was removed from another module and the layout shifted: the overrun moved onto
/// the boot timing table, and twelve of its sixteen entries came back as zero with
/// two holding the bytes `ZPL-FRAME-RX`. Found by decoding one of the nonsense
/// numbers in a `[ZPL-PERF]` line back into ASCII.
///
/// A stack guard does not catch this one: the writes run off the end in a single
/// frame set-up, skipping past the guard page rather than touching it.
///
/// # Safety
/// Written only by `self_check`, which runs once on the boot CPU before anything
/// else can observe it.
static mut SELF_CHECK_BLOCK: MockBlockDevice = MockBlockDevice::new();
static mut SELF_CHECK_NET: Option<MockNetDevice> = None;

pub fn self_check() -> Result<(), SelfCheckErr> {
    // BlockDevice round-trip.
    // SAFETY: see the note on the statics above -- one caller, once, before any
    // other CPU is started.
    let block = unsafe { &mut *core::ptr::addr_of_mut!(SELF_CHECK_BLOCK) };
    *block = MockBlockDevice::new();
    let mut tx = [0u8; BLOCK_SIZE];
    for (i, slot) in tx.iter_mut().enumerate() {
        *slot = (i % 251) as u8;
    }
    block.write_block(2, &tx).map_err(|_| SelfCheckErr::BlockReadbackMismatch)?;
    let mut rx = [0u8; BLOCK_SIZE];
    block.read_block(2, &mut rx).map_err(|_| SelfCheckErr::BlockReadbackMismatch)?;
    if rx != tx {
        return Err(SelfCheckErr::BlockReadbackMismatch);
    }

    // NetDevice round-trip.
    // SAFETY: as above.
    let net_slot = unsafe { &mut *core::ptr::addr_of_mut!(SELF_CHECK_NET) };
    *net_slot = Some(MockNetDevice::new([0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]));
    let net = net_slot.as_mut().expect("just assigned");
    if net.mac_addr() != [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01] {
        return Err(SelfCheckErr::NetMacMismatch);
    }
    let outbound = b"ZPL-FRAME-TX";
    net.send_frame(outbound).map_err(|_| SelfCheckErr::NetTxMismatch)?;
    if net.last_tx_payload() != Some(outbound.as_slice()) {
        return Err(SelfCheckErr::NetTxMismatch);
    }
    let inbound = b"ZPL-FRAME-RX";
    net.inject_rx(inbound).map_err(|_| SelfCheckErr::NetRxMismatch)?;
    let mut buf = [0u8; FRAME_MTU];
    let n = net.recv_frame(&mut buf).map_err(|_| SelfCheckErr::NetRxMismatch)?;
    if &buf[..n] != inbound {
        return Err(SelfCheckErr::NetRxMismatch);
    }
    Ok(())
}
