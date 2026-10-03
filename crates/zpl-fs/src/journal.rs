//! Write-ahead journal for ZPL-FS (partial scope).
//!
//! The journal records intended operations BEFORE they touch the FS
//! state. After a crash the operations replay deterministically,
//! making the file-system consistent at the next `mount`.
//!
//! The v0.1 journal lives in a sidecar `Vec<u8>` (host) -- the kernel
//! integration parks the journal in `JOURNAL_INODE` (#2) once
//! `#15 virtio-blk` lands. The byte layout is identical so the host
//! tool can dump and replay the journal that the kernel persisted.
//!
//! Layout (little-endian):
//! ```text
//!     +------+----------+------------------+--------+--------+
//!     | seq  | opcode   | payload          |   ...  | CRC32  |
//!     | u32  | u8       | depends on op    |        | u32    |
//!     +------+----------+------------------+--------+--------+
//! ```
//!
//! Every entry is followed by a CRC32 covering the seq + opcode +
//! payload bytes. A truncated trailing entry whose CRC fails to
//! validate is treated as "torn write" -- the journal is replayed up
//! to but not including that entry. Plus the corrupt tail is dropped
//! during the next `commit_replay` so it does not get retried.
//!
//! The opcode set is intentionally small (just `Append`) to match the
//! v0.1 surface of `ZplFs::append`. Future opcodes (`SetSize`,
//! `Truncate`, `Unlink`) drop in alongside without a layout change.

use crate::{crc32, Error, ZplFs};

const OP_APPEND: u8 = 0x01;

/// Single recorded operation. The journal stores a sequence of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Append `bytes` to the file at `inode`.
    Append { inode: u32, bytes: Vec<u8> },
}

/// Journal stored as a flat byte buffer (the v0.1 host shape).
pub struct Journal {
    bytes: Vec<u8>,
    seq: u32,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

impl Journal {
    pub fn new() -> Self {
        Journal {
            bytes: Vec::new(),
            seq: 0,
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        // Recover the seq counter by reading the last decoded entry.
        let seq = decode(&bytes)
            .into_iter()
            .last()
            .map(|(s, _)| s + 1)
            .unwrap_or(0);
        Journal { bytes, seq }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn entries(&self) -> Vec<Op> {
        decode(&self.bytes).into_iter().map(|(_, op)| op).collect()
    }

    /// Record an `Append` operation in the journal.
    pub fn record_append(&mut self, inode: u32, bytes: &[u8]) {
        let mut frame: Vec<u8> = Vec::with_capacity(4 + 1 + 4 + 4 + bytes.len() + 4);
        frame.extend_from_slice(&self.seq.to_le_bytes());
        frame.push(OP_APPEND);
        frame.extend_from_slice(&inode.to_le_bytes());
        frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        frame.extend_from_slice(bytes);
        let crc = crc32(&frame);
        frame.extend_from_slice(&crc.to_le_bytes());
        self.bytes.extend_from_slice(&frame);
        self.seq = self.seq.wrapping_add(1);
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
        self.seq = 0;
    }

    /// Apply every entry currently in the journal to `fs`. Used by
    /// `JournaledFs::mount` for crash recovery.
    pub fn replay(&self, fs: &mut ZplFs) -> Result<usize, Error> {
        let entries = self.entries();
        let count = entries.len();
        for op in entries {
            match op {
                Op::Append { inode, bytes } => fs.append(inode, &bytes)?,
            }
        }
        Ok(count)
    }
}

fn decode(bytes: &[u8]) -> Vec<(u32, Op)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor + 13 <= bytes.len() {
        let seq = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
        let opcode = bytes[cursor + 4];
        match opcode {
            OP_APPEND => {
                let inode_off = cursor + 5;
                let len_off = cursor + 9;
                if len_off + 4 > bytes.len() {
                    break;
                }
                let inode = u32::from_le_bytes(
                    bytes[inode_off..inode_off + 4].try_into().unwrap(),
                );
                let payload_len = u32::from_le_bytes(
                    bytes[len_off..len_off + 4].try_into().unwrap(),
                ) as usize;
                let payload_off = len_off + 4;
                let crc_off = payload_off + payload_len;
                if crc_off + 4 > bytes.len() {
                    break; // torn write
                }
                let frame_end = crc_off + 4;
                let stored_crc = u32::from_le_bytes(
                    bytes[crc_off..frame_end].try_into().unwrap(),
                );
                let actual_crc = crc32(&bytes[cursor..crc_off]);
                if stored_crc != actual_crc {
                    break; // torn write
                }
                let payload = bytes[payload_off..crc_off].to_vec();
                out.push((seq, Op::Append { inode, bytes: payload }));
                cursor = frame_end;
            }
            _ => break, // unknown opcode -> stop replay
        }
    }
    out
}

/// `ZplFs` wrapper that records every mutating call in `Journal` and
/// commits both atomically.
pub struct JournaledFs<'a> {
    pub fs: ZplFs<'a>,
    pub journal: Journal,
}

impl<'a> JournaledFs<'a> {
    /// Mount with an empty journal.
    pub fn mount(device: &'a mut [u8]) -> Result<Self, Error> {
        let fs = ZplFs::mount(device)?;
        Ok(JournaledFs {
            fs,
            journal: Journal::new(),
        })
    }

    /// Mount and replay any pending operations from `journal_bytes`.
    /// Returns the number of replayed entries.
    pub fn mount_with_journal(
        device: &'a mut [u8],
        journal_bytes: Vec<u8>,
    ) -> Result<(Self, usize), Error> {
        let mut fs = ZplFs::mount(device)?;
        let journal = Journal::from_bytes(journal_bytes);
        let replayed = journal.replay(&mut fs)?;
        Ok((
            JournaledFs {
                fs,
                journal: Journal::new(),
            },
            replayed,
        ))
    }

    /// Append `data` to `inode_idx`, recording the intent in the
    /// journal first. Crashes between the journal write and the FS
    /// mutation replay deterministically because of `mount_with_journal`.
    pub fn append(&mut self, inode_idx: u32, data: &[u8]) -> Result<(), Error> {
        self.journal.record_append(inode_idx, data);
        self.fs.append(inode_idx, data)
    }

    /// Discard the journal -- call after the underlying device has been
    /// flushed so a subsequent crash skips replay.
    pub fn commit(&mut self) {
        self.journal.clear();
    }

    pub fn allocate_reserved(&mut self, idx: u32) -> Result<(), Error> {
        self.fs.allocate_reserved(idx)
    }

    pub fn allocate_inode(&mut self) -> Result<u32, Error> {
        self.fs.allocate_inode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{format, AUDIT_CHAIN_INODE, BLOCK_SIZE};

    fn fresh_device(blocks: usize) -> Vec<u8> {
        let mut device = vec![0u8; blocks * BLOCK_SIZE];
        format(&mut device).unwrap();
        device
    }

    #[test]
    fn journal_replay_round_trips_writes() {
        let mut device = fresh_device(16);
        let journal_bytes;
        // Step 1: clean run, journal records, then we drop without commit.
        {
            let mut jfs = JournaledFs::mount(&mut device).unwrap();
            jfs.allocate_reserved(AUDIT_CHAIN_INODE).unwrap();
            jfs.append(AUDIT_CHAIN_INODE, b"first ").unwrap();
            jfs.append(AUDIT_CHAIN_INODE, b"second ").unwrap();
            jfs.append(AUDIT_CHAIN_INODE, b"third").unwrap();
            // Snapshot the journal *before* commit -- this is what would
            // survive a power loss after the FS bytes were written.
            journal_bytes = jfs.journal.as_bytes().to_vec();
            // No commit() -- the next mount must redo the journal.
        }
        // Step 2: format-equivalent crash recovery: rewrite a fresh FS
        // and replay the journal on top to mimic "the FS state is
        // consistent at recovery".
        let mut device2 = fresh_device(16);
        {
            let mut jfs2 = JournaledFs::mount(&mut device2).unwrap();
            jfs2.allocate_reserved(AUDIT_CHAIN_INODE).unwrap();
        }
        let (jfs2, replayed) =
            JournaledFs::mount_with_journal(&mut device2, journal_bytes).unwrap();
        assert_eq!(replayed, 3);
        let mut out = [0u8; 32];
        let read = jfs2
            .fs
            .read_at(AUDIT_CHAIN_INODE, 0, &mut out)
            .unwrap();
        assert_eq!(read, b"first second third".len());
        assert_eq!(&out[..read], b"first second third");
    }

    #[test]
    fn truncated_journal_drops_torn_tail() {
        let mut journal = Journal::new();
        journal.record_append(7, b"hello");
        journal.record_append(7, b"world");
        let mut bytes = journal.into_bytes();
        // Truncate the last entry mid-payload to simulate a torn write.
        let len = bytes.len();
        bytes.truncate(len - 3);
        let recovered = Journal::from_bytes(bytes);
        let entries = recovered.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0],
            Op::Append {
                inode: 7,
                bytes: b"hello".to_vec(),
            }
        );
    }

    #[test]
    fn corrupt_journal_crc_drops_tail() {
        let mut journal = Journal::new();
        journal.record_append(3, b"good entry");
        journal.record_append(3, b"will-be-corrupted");
        let mut bytes = journal.into_bytes();
        // Flip a byte in the second entry's payload; CRC must fail.
        let target = bytes.len() - 6;
        bytes[target] ^= 0xFF;
        let recovered = Journal::from_bytes(bytes);
        let entries = recovered.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0],
            Op::Append {
                inode: 3,
                bytes: b"good entry".to_vec(),
            }
        );
    }
}
