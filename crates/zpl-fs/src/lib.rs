//! ZPL-FS v0.1 codec (partial scope).
//!
//! Pure-byte filesystem codec that operates on a flat `&mut [u8]`
//! device. The kernel-side wrapper that talks to a real virtio-blk
//! disk lands with the block-device driver; until then this codec is exercised by
//! the `zpl-mkfs` host tool and by future audit-chain persistence
//! (the audit chain's long-term home).
//!
//! On-disk layout (all little-endian, 4 KiB blocks):
//! ```text
//!     +---------------------+ block 0
//!     | Superblock          |  magic + version + bitmap (max 1 MiB)
//!     +---------------------+ block 1
//!     | Inode table         |  64 inode slots * 64 bytes = 4 KiB
//!     +---------------------+ blocks 2..N
//!     | Data blocks         |  first-fit bitmap-allocated
//!     +---------------------+
//! ```
//!
//! Constraints (v0.1):
//! - Single flat namespace (no directories), files indexed by `inode_id`.
//! - 64 inode slots; reserved IDs 0-3 are kernel use (root, audit chain,
//!   future trace ring, future config).
//! - 12 direct block pointers per inode; max file size = 48 KiB.
//! - 4 KiB blocks; 256-block (1 MiB) maximum image.
//! - CRC-32 on every superblock + inode + data block.
//!
//! Why this minimal shape: it covers the task acceptance ("create
//! file, write 4 KiB, read back, integrity OK") and the audit-chain
//! persistence story without re-litigating ext2's design.
//! Indirect blocks, directory inodes, and >1 MiB images are explicit
//! follow-up work.

#![forbid(unsafe_code)]

mod crc;
pub mod journal;

pub use crc::crc32;
pub use journal::{JournaledFs, Journal, Op};

pub const MAGIC: u64 = 0x5A50_4C46_5330_3031; // "ZPLFS001"
pub const VERSION: u32 = 1;
pub const BLOCK_SIZE: usize = 4096;
pub const INODE_SIZE: usize = 64;
pub const INODES_PER_BLOCK: usize = BLOCK_SIZE / INODE_SIZE;
pub const INODE_COUNT: usize = INODES_PER_BLOCK; // single-block inode table
/// Eleven direct block pointers per inode. The 12th `u32` slot is
/// reused for the trailing CRC32 of the inode itself, so the layout
/// stays exactly 64 bytes:
/// `kind(1) + pad(7) + size(8) + 11*block(44) + crc32(4) = 64`.
pub const DIRECT_BLOCKS: usize = 11;
/// Bytes of user payload carried by each data block (the trailing 4
/// bytes of every data block hold the CRC).
pub const PAYLOAD_PER_BLOCK: usize = BLOCK_SIZE - 4;
pub const SUPERBLOCK_BLOCK: usize = 0;
pub const INODE_TABLE_BLOCK: usize = 1;
pub const DATA_BLOCK_START: usize = 2;
pub const MAX_BLOCKS: usize = 256; // 256 * 4 KiB = 1 MiB image
pub const RESERVED_INODES: u32 = 4;
pub const ROOT_INODE: u32 = 0;
pub const AUDIT_CHAIN_INODE: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// What an inode slot holds. There are no directories yet, so a slot is either
/// free or a file.
pub enum InodeKind {
    /// The slot is unused and may be allocated.
    Free = 0,
    /// The slot holds a file.
    File = 1,
}

impl InodeKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(InodeKind::Free),
            1 => Some(InodeKind::File),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Everything that can go wrong reading or writing a ZPL-FS image.
///
/// The first six are "this is not a filesystem I can use"; the `Bad*Crc` pair is
/// "it is, and it has been damaged"; the rest are ordinary refusals.
pub enum Error {
    /// The image is smaller than a superblock plus one inode table.
    DeviceTooSmall,
    /// The image is larger than the block count a superblock can address.
    DeviceTooLarge,
    /// The image length is not a whole number of blocks.
    DeviceMisaligned,
    /// The first eight bytes are not the ZPL-FS magic. Probably not our format.
    BadMagic,
    /// The magic matches but the version is one this code does not know.
    BadVersion,
    /// The superblock's checksum does not match its contents.
    BadSuperblockCrc,
    /// An inode's checksum does not match its contents.
    BadInodeCrc {
        /// Index of the inode that failed.
        inode: u32,
    },
    /// A data block's checksum does not match its contents.
    BadBlockCrc {
        /// Index of the block that failed.
        block: usize,
    },
    /// The inode number is past the end of the inode table.
    InodeOutOfRange,
    /// The inode exists but is not allocated.
    InodeFree,
    /// The inode is already in use and was asked to be allocated again.
    InodeAlreadyAllocated,
    /// The inode table is full.
    NoFreeInodes,
    /// The data area is full.
    NoFreeBlocks,
    /// The read or write starts past the end of the file.
    OffsetOutOfRange,
    /// The read or write runs past what the caller's buffer can hold.
    LengthOutOfRange,
    /// The file would grow past the largest size an inode can describe.
    FileTooLarge,
}

#[derive(Debug, Clone, Copy)]
struct Superblock {
    magic: u64,
    version: u32,
    block_size: u32,
    block_count: u32,
    inode_count: u32,
    inode_table_block: u32,
    data_block_start: u32,
    bitmap: [u8; 64], // bit per data block, max 512 data blocks (we cap at 256 total -> 254 data)
    crc: u32,
}

impl Superblock {
    fn fresh(block_count: u32) -> Self {
        Superblock {
            magic: MAGIC,
            version: VERSION,
            block_size: BLOCK_SIZE as u32,
            block_count,
            inode_count: INODE_COUNT as u32,
            inode_table_block: INODE_TABLE_BLOCK as u32,
            data_block_start: DATA_BLOCK_START as u32,
            bitmap: [0u8; 64],
            crc: 0,
        }
    }

    fn serialize_no_crc(&self, buf: &mut [u8; BLOCK_SIZE]) {
        buf.fill(0);
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..12].copy_from_slice(&self.version.to_le_bytes());
        buf[12..16].copy_from_slice(&self.block_size.to_le_bytes());
        buf[16..20].copy_from_slice(&self.block_count.to_le_bytes());
        buf[20..24].copy_from_slice(&self.inode_count.to_le_bytes());
        buf[24..28].copy_from_slice(&self.inode_table_block.to_le_bytes());
        buf[28..32].copy_from_slice(&self.data_block_start.to_le_bytes());
        buf[32..96].copy_from_slice(&self.bitmap);
        // crc lives at bytes 4092..4096
    }

    fn write(&mut self, buf: &mut [u8; BLOCK_SIZE]) {
        self.serialize_no_crc(buf);
        let crc = crc32(&buf[..BLOCK_SIZE - 4]);
        self.crc = crc;
        buf[BLOCK_SIZE - 4..BLOCK_SIZE].copy_from_slice(&crc.to_le_bytes());
    }

    fn read(buf: &[u8; BLOCK_SIZE]) -> Result<Self, Error> {
        let stored_crc = u32::from_le_bytes([
            buf[BLOCK_SIZE - 4],
            buf[BLOCK_SIZE - 3],
            buf[BLOCK_SIZE - 2],
            buf[BLOCK_SIZE - 1],
        ]);
        let actual_crc = crc32(&buf[..BLOCK_SIZE - 4]);
        if stored_crc != actual_crc {
            return Err(Error::BadSuperblockCrc);
        }
        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        if magic != MAGIC {
            return Err(Error::BadMagic);
        }
        let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        if version != VERSION {
            return Err(Error::BadVersion);
        }
        let mut bitmap = [0u8; 64];
        bitmap.copy_from_slice(&buf[32..96]);
        Ok(Superblock {
            magic,
            version,
            block_size: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            block_count: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            inode_count: u32::from_le_bytes(buf[20..24].try_into().unwrap()),
            inode_table_block: u32::from_le_bytes(buf[24..28].try_into().unwrap()),
            data_block_start: u32::from_le_bytes(buf[28..32].try_into().unwrap()),
            bitmap,
            crc: stored_crc,
        })
    }

    fn alloc_block(&mut self) -> Option<u32> {
        for byte_idx in 0..self.bitmap.len() {
            let byte = self.bitmap[byte_idx];
            if byte == 0xFF {
                continue;
            }
            for bit in 0..8 {
                if byte & (1 << bit) == 0 {
                    let abs = byte_idx * 8 + bit;
                    let block_idx = self.data_block_start as usize + abs;
                    if block_idx >= self.block_count as usize {
                        return None;
                    }
                    self.bitmap[byte_idx] |= 1 << bit;
                    return Some(block_idx as u32);
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Inode {
    kind: InodeKind,
    size: u64,
    blocks: [u32; DIRECT_BLOCKS],
    crc: u32,
}

impl Inode {
    fn empty() -> Self {
        Inode {
            kind: InodeKind::Free,
            size: 0,
            blocks: [0; DIRECT_BLOCKS],
            crc: 0,
        }
    }

    fn write(&mut self, buf: &mut [u8; INODE_SIZE]) {
        buf.fill(0);
        buf[0] = self.kind as u8;
        buf[8..16].copy_from_slice(&self.size.to_le_bytes());
        for (i, blk) in self.blocks.iter().enumerate() {
            let off = 16 + i * 4;
            buf[off..off + 4].copy_from_slice(&blk.to_le_bytes());
        }
        // crc bytes at 60..64
        let crc = crc32(&buf[..INODE_SIZE - 4]);
        self.crc = crc;
        buf[INODE_SIZE - 4..INODE_SIZE].copy_from_slice(&crc.to_le_bytes());
    }

    fn read(buf: &[u8; INODE_SIZE], idx: u32) -> Result<Self, Error> {
        let stored_crc = u32::from_le_bytes(
            buf[INODE_SIZE - 4..INODE_SIZE].try_into().unwrap(),
        );
        let actual_crc = crc32(&buf[..INODE_SIZE - 4]);
        if stored_crc != actual_crc {
            return Err(Error::BadInodeCrc { inode: idx });
        }
        let kind = InodeKind::from_u8(buf[0]).ok_or(Error::BadInodeCrc { inode: idx })?;
        let size = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let mut blocks = [0u32; DIRECT_BLOCKS];
        for (i, slot) in blocks.iter_mut().enumerate() {
            let off = 16 + i * 4;
            *slot = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
        }
        Ok(Inode {
            kind,
            size,
            blocks,
            crc: stored_crc,
        })
    }
}

/// Format a freshly-zeroed device with an empty ZPL-FS image.
pub fn format(device: &mut [u8]) -> Result<(), Error> {
    if !device.len().is_multiple_of(BLOCK_SIZE) {
        return Err(Error::DeviceMisaligned);
    }
    let block_count = device.len() / BLOCK_SIZE;
    if block_count < DATA_BLOCK_START + 1 {
        return Err(Error::DeviceTooSmall);
    }
    if block_count > MAX_BLOCKS {
        return Err(Error::DeviceTooLarge);
    }

    device.fill(0);

    let mut sb = Superblock::fresh(block_count as u32);
    let mut sb_buf = [0u8; BLOCK_SIZE];
    sb.write(&mut sb_buf);
    write_block(device, SUPERBLOCK_BLOCK, &sb_buf);

    let mut inode_buf = [0u8; BLOCK_SIZE];
    for slot in 0..INODE_COUNT {
        let mut inode = Inode::empty();
        let mut slot_buf = [0u8; INODE_SIZE];
        inode.write(&mut slot_buf);
        let off = slot * INODE_SIZE;
        inode_buf[off..off + INODE_SIZE].copy_from_slice(&slot_buf);
    }
    write_block(device, INODE_TABLE_BLOCK, &inode_buf);
    Ok(())
}

/// Top-level handle. Reads superblock + inode table eagerly; data
/// blocks are read on demand.
pub struct ZplFs<'a> {
    device: &'a mut [u8],
    sb: Superblock,
}

impl<'a> ZplFs<'a> {
    pub fn mount(device: &'a mut [u8]) -> Result<Self, Error> {
        if !device.len().is_multiple_of(BLOCK_SIZE) {
            return Err(Error::DeviceMisaligned);
        }
        let mut sb_buf = [0u8; BLOCK_SIZE];
        read_block(device, SUPERBLOCK_BLOCK, &mut sb_buf);
        let sb = Superblock::read(&sb_buf)?;
        Ok(ZplFs { device, sb })
    }

    pub fn block_count(&self) -> u32 {
        self.sb.block_count
    }

    pub fn flush_superblock(&mut self) {
        let mut buf = [0u8; BLOCK_SIZE];
        self.sb.write(&mut buf);
        write_block(self.device, SUPERBLOCK_BLOCK, &buf);
    }

    fn read_inode(&self, idx: u32) -> Result<Inode, Error> {
        if idx >= self.sb.inode_count {
            return Err(Error::InodeOutOfRange);
        }
        let mut buf = [0u8; BLOCK_SIZE];
        read_block(self.device, self.sb.inode_table_block as usize, &mut buf);
        let off = idx as usize * INODE_SIZE;
        let mut slot_buf = [0u8; INODE_SIZE];
        slot_buf.copy_from_slice(&buf[off..off + INODE_SIZE]);
        Inode::read(&slot_buf, idx)
    }

    fn write_inode(&mut self, idx: u32, inode: &mut Inode) -> Result<(), Error> {
        if idx >= self.sb.inode_count {
            return Err(Error::InodeOutOfRange);
        }
        let mut buf = [0u8; BLOCK_SIZE];
        read_block(self.device, self.sb.inode_table_block as usize, &mut buf);
        let off = idx as usize * INODE_SIZE;
        let mut slot_buf = [0u8; INODE_SIZE];
        inode.write(&mut slot_buf);
        buf[off..off + INODE_SIZE].copy_from_slice(&slot_buf);
        write_block(self.device, self.sb.inode_table_block as usize, &buf);
        Ok(())
    }

    /// Allocate a fresh inode slot. Reserved IDs (`< RESERVED_INODES`)
    /// must be allocated explicitly via `allocate_reserved`.
    pub fn allocate_inode(&mut self) -> Result<u32, Error> {
        for idx in RESERVED_INODES..self.sb.inode_count {
            let inode = self.read_inode(idx)?;
            if matches!(inode.kind, InodeKind::Free) {
                let mut new_inode = Inode {
                    kind: InodeKind::File,
                    size: 0,
                    blocks: [0; DIRECT_BLOCKS],
                    crc: 0,
                };
                self.write_inode(idx, &mut new_inode)?;
                return Ok(idx);
            }
        }
        Err(Error::NoFreeInodes)
    }

    /// Reserve a specific inode ID (typically `ROOT_INODE` or
    /// `AUDIT_CHAIN_INODE`).
    pub fn allocate_reserved(&mut self, idx: u32) -> Result<(), Error> {
        if idx >= RESERVED_INODES {
            return Err(Error::InodeOutOfRange);
        }
        let inode = self.read_inode(idx)?;
        if !matches!(inode.kind, InodeKind::Free) {
            return Err(Error::InodeAlreadyAllocated);
        }
        let mut new_inode = Inode {
            kind: InodeKind::File,
            size: 0,
            blocks: [0; DIRECT_BLOCKS],
            crc: 0,
        };
        self.write_inode(idx, &mut new_inode)?;
        Ok(())
    }

    /// Append `data` to the end of the file at `inode_idx`.
    pub fn append(&mut self, inode_idx: u32, data: &[u8]) -> Result<(), Error> {
        let mut inode = self.read_inode(inode_idx)?;
        if !matches!(inode.kind, InodeKind::File) {
            return Err(Error::InodeFree);
        }

        let mut written = 0usize;
        while written < data.len() {
            let file_off = inode.size as usize + written;
            let block_in_file = file_off / PAYLOAD_PER_BLOCK;
            if block_in_file >= DIRECT_BLOCKS {
                return Err(Error::FileTooLarge);
            }
            let block_off = file_off % PAYLOAD_PER_BLOCK;

            let block_idx = if inode.blocks[block_in_file] == 0 {
                let new_block = self.sb.alloc_block().ok_or(Error::NoFreeBlocks)?;
                inode.blocks[block_in_file] = new_block;
                self.flush_superblock();
                new_block
            } else {
                inode.blocks[block_in_file]
            };

            let mut buf = [0u8; BLOCK_SIZE];
            read_block(self.device, block_idx as usize, &mut buf);
            let chunk = (PAYLOAD_PER_BLOCK - block_off).min(data.len() - written);
            buf[block_off..block_off + chunk]
                .copy_from_slice(&data[written..written + chunk]);
            // CRC over first PAYLOAD_PER_BLOCK bytes; tail 4 bytes hold the CRC.
            let crc = crc32(&buf[..PAYLOAD_PER_BLOCK]);
            buf[PAYLOAD_PER_BLOCK..BLOCK_SIZE].copy_from_slice(&crc.to_le_bytes());
            write_block(self.device, block_idx as usize, &buf);

            written += chunk;
        }

        inode.size += data.len() as u64;
        self.write_inode(inode_idx, &mut inode)?;
        Ok(())
    }

    /// Read `len` bytes from `inode_idx` starting at `offset` into
    /// `out`. Returns the number of bytes actually copied.
    pub fn read_at(
        &self,
        inode_idx: u32,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, Error> {
        let inode = self.read_inode(inode_idx)?;
        if !matches!(inode.kind, InodeKind::File) {
            return Err(Error::InodeFree);
        }
        if offset > inode.size {
            return Err(Error::OffsetOutOfRange);
        }
        let available = (inode.size - offset) as usize;
        let to_copy = available.min(out.len());
        let mut copied = 0usize;
        while copied < to_copy {
            let file_off = offset as usize + copied;
            let block_in_file = file_off / PAYLOAD_PER_BLOCK;
            let block_off = file_off % PAYLOAD_PER_BLOCK;
            if block_in_file >= DIRECT_BLOCKS {
                return Err(Error::OffsetOutOfRange);
            }
            let block_idx = inode.blocks[block_in_file];
            if block_idx == 0 {
                return Err(Error::OffsetOutOfRange);
            }
            let mut buf = [0u8; BLOCK_SIZE];
            read_block(self.device, block_idx as usize, &mut buf);
            let stored_crc = u32::from_le_bytes(
                buf[PAYLOAD_PER_BLOCK..BLOCK_SIZE].try_into().unwrap(),
            );
            let actual_crc = crc32(&buf[..PAYLOAD_PER_BLOCK]);
            if stored_crc != actual_crc {
                return Err(Error::BadBlockCrc { block: block_idx as usize });
            }
            let chunk = (PAYLOAD_PER_BLOCK - block_off).min(to_copy - copied);
            out[copied..copied + chunk]
                .copy_from_slice(&buf[block_off..block_off + chunk]);
            copied += chunk;
        }
        Ok(copied)
    }

    pub fn file_size(&self, inode_idx: u32) -> Result<u64, Error> {
        let inode = self.read_inode(inode_idx)?;
        if !matches!(inode.kind, InodeKind::File) {
            return Err(Error::InodeFree);
        }
        Ok(inode.size)
    }

    /// Verify the integrity of the entire image: superblock CRC,
    /// every inode CRC, every used data block CRC.
    pub fn fsck(&self) -> Result<FsckReport, Error> {
        let mut report = FsckReport {
            block_count: self.sb.block_count as usize,
            ..FsckReport::default()
        };
        // superblock already verified by mount; recheck for fsck to be
        // explicit
        let mut sb_buf = [0u8; BLOCK_SIZE];
        read_block(self.device, SUPERBLOCK_BLOCK, &mut sb_buf);
        let _ = Superblock::read(&sb_buf)?;

        for idx in 0..self.sb.inode_count {
            let inode = self.read_inode(idx)?;
            if matches!(inode.kind, InodeKind::File) {
                report.inode_count += 1;
                report.bytes_total += inode.size as usize;
                // verify each used data block CRC
                for block_idx in inode.blocks.iter().filter(|b| **b != 0) {
                    let mut buf = [0u8; BLOCK_SIZE];
                    read_block(self.device, *block_idx as usize, &mut buf);
                    let stored = u32::from_le_bytes(
                        buf[PAYLOAD_PER_BLOCK..BLOCK_SIZE].try_into().unwrap(),
                    );
                    let actual = crc32(&buf[..PAYLOAD_PER_BLOCK]);
                    if stored != actual {
                        return Err(Error::BadBlockCrc {
                            block: *block_idx as usize,
                        });
                    }
                    report.data_blocks_used += 1;
                }
            }
        }
        Ok(report)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FsckReport {
    pub block_count: usize,
    pub inode_count: usize,
    pub data_blocks_used: usize,
    pub bytes_total: usize,
}

fn read_block(device: &[u8], block_idx: usize, out: &mut [u8; BLOCK_SIZE]) {
    let off = block_idx * BLOCK_SIZE;
    out.copy_from_slice(&device[off..off + BLOCK_SIZE]);
}

fn write_block(device: &mut [u8], block_idx: usize, data: &[u8; BLOCK_SIZE]) {
    let off = block_idx * BLOCK_SIZE;
    device[off..off + BLOCK_SIZE].copy_from_slice(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_device(blocks: usize) -> Vec<u8> {
        let mut device = vec![0u8; blocks * BLOCK_SIZE];
        format(&mut device).unwrap();
        device
    }

    #[test]
    fn format_then_mount_round_trips_superblock() {
        let mut device = fresh_device(8);
        let fs = ZplFs::mount(&mut device).unwrap();
        assert_eq!(fs.block_count(), 8);
    }

    #[test]
    fn allocate_reserved_then_append_then_read() {
        let mut device = fresh_device(16);
        {
            let mut fs = ZplFs::mount(&mut device).unwrap();
            fs.allocate_reserved(AUDIT_CHAIN_INODE).unwrap();
            fs.append(AUDIT_CHAIN_INODE, b"hello world").unwrap();
            fs.append(AUDIT_CHAIN_INODE, b" plus more bytes").unwrap();
            assert_eq!(fs.file_size(AUDIT_CHAIN_INODE).unwrap(), 27);
        }
        // simulate "reboot" by re-mounting from the same buffer
        let fs = ZplFs::mount(&mut device).unwrap();
        let mut out = [0u8; 64];
        let read = fs.read_at(AUDIT_CHAIN_INODE, 0, &mut out).unwrap();
        assert_eq!(read, 27);
        assert_eq!(&out[..read], b"hello world plus more bytes");
    }

    #[test]
    fn write_4kib_round_trip_integrity() {
        let mut device = fresh_device(16);
        let payload: Vec<u8> = (0..4096u32).map(|i| (i & 0xff) as u8).collect();
        let inode_idx;
        {
            let mut fs = ZplFs::mount(&mut device).unwrap();
            inode_idx = fs.allocate_inode().unwrap();
            // Each block holds BLOCK_SIZE-4 = 4092 user bytes.
            // Append the full 4 KiB; spans 2 blocks.
            fs.append(inode_idx, &payload).unwrap();
            assert_eq!(fs.file_size(inode_idx).unwrap(), 4096);
        }
        let fs = ZplFs::mount(&mut device).unwrap();
        let mut out = vec![0u8; 4096];
        let read = fs.read_at(inode_idx, 0, &mut out).unwrap();
        assert_eq!(read, 4096);
        assert_eq!(out, payload);
        let report = fs.fsck().unwrap();
        assert_eq!(report.inode_count, 1);
        assert!(report.data_blocks_used >= 2);
    }

    #[test]
    fn fsck_detects_data_block_tampering() {
        let mut device = fresh_device(16);
        {
            let mut fs = ZplFs::mount(&mut device).unwrap();
            let idx = fs.allocate_inode().unwrap();
            fs.append(idx, b"important payload").unwrap();
        }
        // flip a byte in the data block (block 2 onwards)
        let target = 2 * BLOCK_SIZE + 5;
        device[target] ^= 0xFF;
        let fs = ZplFs::mount(&mut device).unwrap();
        match fs.fsck() {
            Err(Error::BadBlockCrc { block: 2 }) => {}
            other => panic!("expected BadBlockCrc {{ block: 2 }}, got {:?}", other),
        }
    }

    #[test]
    fn read_at_offset_into_partial_buffer() {
        let mut device = fresh_device(16);
        let payload: Vec<u8> = (0..200u32).map(|i| i as u8).collect();
        let idx;
        {
            let mut fs = ZplFs::mount(&mut device).unwrap();
            idx = fs.allocate_inode().unwrap();
            fs.append(idx, &payload).unwrap();
        }
        let fs = ZplFs::mount(&mut device).unwrap();
        let mut out = [0u8; 50];
        let read = fs.read_at(idx, 100, &mut out).unwrap();
        assert_eq!(read, 50);
        for (i, byte) in out.iter().enumerate() {
            assert_eq!(*byte, ((100 + i) as u8));
        }
    }

    #[test]
    fn no_free_inodes_when_table_is_full() {
        let mut device = fresh_device(16);
        let mut fs = ZplFs::mount(&mut device).unwrap();
        for _ in 0..(INODE_COUNT - RESERVED_INODES as usize) {
            fs.allocate_inode().unwrap();
        }
        match fs.allocate_inode() {
            Err(Error::NoFreeInodes) => {}
            other => panic!("expected NoFreeInodes, got {:?}", other),
        }
    }
}
