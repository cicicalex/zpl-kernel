//! Hash-chained audit log of kernel decisions (partial scope).
//!
//! Each kernel decision (today: `sys_compute` outcome) is appended as a
//! `AuditRecord`. Every record carries a non-cryptographic mix of
//! its predecessor so the chain can be verified end-to-end by
//! `verify_chain`.
//!
//! Limitations of v0.1:
//! - **In-RAM only.** Persistence to ZPL-FS lands when task `#16`
//!   ships `virtio-blk` + on-disk format. The audit-replay tool
//!   consumes the same record layout but reads it from the
//!   on-disk file once that lands.
//! - **Non-cryptographic hash.** A constant-time bit-mixer is deterministic
//!   and fast but is not collision-resistant. Ed25519 signing wraps
//!   each record once `#54` (license) and the upgraded
//!   `pb-core-nostd` signing surface land. The structural integrity
//!   gate ("re-hash from genesis matches stored hashes") is what
//!   v0.1 catches today.
//!
//! The score recorded here comes from the policy gateway; nothing about
//! how it is produced is visible from this module.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::sync::atomic::{AtomicBool, Ordering};

extern crate alloc;
use alloc::vec::Vec;

const GENESIS_HASH: u64 = 0xDEAD_BEEF_CAFE_BABE;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AuditRecord {
    pub seq: u64,
    pub tick: u64,
    pub tid: u32,
    pub ain_pct: u8,
    pub decision: u8,
    pub prev_hash: u64,
    pub self_hash: u64,
}

impl AuditRecord {
    pub fn body_for_hash(&self) -> [u64; 4] {
        [
            self.seq,
            self.tick,
            ((self.tid as u64) << 16) | ((self.ain_pct as u64) << 8) | (self.decision as u64),
            self.prev_hash,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditError {
    NotInitialized,
    Mismatch { at: usize },
}

static INIT_DONE: AtomicBool = AtomicBool::new(false);
static mut CHAIN: Option<Vec<AuditRecord>> = None;

/// Reset the chain. Call once during boot (after `heap::init`).
///
/// # Safety
/// Caller must guarantee no other thread mutates the chain.
pub unsafe fn init() {
    unsafe {
        let ptr = (&raw mut CHAIN) as *mut Option<Vec<AuditRecord>>;
        *ptr = Some(Vec::new());
    }
    INIT_DONE.store(true, Ordering::SeqCst);
}

#[inline]
fn check_init() -> Result<(), AuditError> {
    if INIT_DONE.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(AuditError::NotInitialized)
    }
}

#[inline]
fn mix(state: u64) -> u64 {
    // Constant-time bit-mixer (one of the public Vigna-style
    // finalizers); deterministic and adequate for v0.1
    // structural integrity. NOT collision-resistant.
    let mut z = state.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn record_hash(body: &[u64; 4]) -> u64 {
    let mut acc = GENESIS_HASH;
    for word in body.iter() {
        acc = mix(acc ^ *word);
    }
    acc
}

/// Append a decision record to the chain.
pub fn append(tick: u64, tid: u32, ain_pct: u8, decision: u8) -> Result<u64, AuditError> {
    check_init()?;
    unsafe {
        let chain_ref = (&raw mut CHAIN) as *mut Option<Vec<AuditRecord>>;
        let chain = (*chain_ref).as_mut().ok_or(AuditError::NotInitialized)?;
        let seq = chain.len() as u64;
        let prev_hash = chain.last().map(|r| r.self_hash).unwrap_or(GENESIS_HASH);
        let mut record = AuditRecord {
            seq,
            tick,
            tid,
            ain_pct,
            decision,
            prev_hash,
            self_hash: 0,
        };
        record.self_hash = record_hash(&record.body_for_hash());
        chain.push(record);
        Ok(record.self_hash)
    }
}

/// Re-hash the chain from genesis and assert each record's
/// `self_hash` matches. Returns the index that mismatched on failure.
pub fn verify_chain() -> Result<usize, AuditError> {
    check_init()?;
    unsafe {
        let chain_ref = (&raw const CHAIN) as *const Option<Vec<AuditRecord>>;
        let chain = (*chain_ref).as_ref().ok_or(AuditError::NotInitialized)?;
        let mut prev = GENESIS_HASH;
        for (idx, record) in chain.iter().enumerate() {
            if record.prev_hash != prev {
                return Err(AuditError::Mismatch { at: idx });
            }
            let expected = record_hash(&record.body_for_hash());
            if expected != record.self_hash {
                return Err(AuditError::Mismatch { at: idx });
            }
            prev = record.self_hash;
        }
        Ok(chain.len())
    }
}

pub fn len() -> usize {
    unsafe {
        let chain_ref = (&raw const CHAIN) as *const Option<Vec<AuditRecord>>;
        match (*chain_ref).as_ref() {
            Some(chain) => chain.len(),
            None => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    AppendFailed,
    VerifyFailed,
    LenWrong { expected: usize, got: usize },
    TamperUndetected,
}

pub fn self_check() -> Result<(), SelfCheckErr> {
    for tick in 0..5u64 {
        let tid = (tick % 3) as u32;
        let ain_pct = ((tick * 13) % 100) as u8;
        let decision = (tick % 3) as u8; // ALLOW=0, DEGRADE=1, BLOCK=2
        append(tick, tid, ain_pct, decision).map_err(|_| SelfCheckErr::AppendFailed)?;
    }
    let count = verify_chain().map_err(|_| SelfCheckErr::VerifyFailed)?;
    if count != 5 {
        return Err(SelfCheckErr::LenWrong {
            expected: 5,
            got: count,
        });
    }

    // Tamper test: corrupt a record, re-verify, expect failure.
    unsafe {
        let chain_ref = (&raw mut CHAIN) as *mut Option<Vec<AuditRecord>>;
        if let Some(chain) = (*chain_ref).as_mut() {
            chain[2].ain_pct ^= 0x01;
            match verify_chain() {
                Ok(_) => return Err(SelfCheckErr::TamperUndetected),
                Err(_) => {}
            }
            // Restore so subsequent boot phases see a clean chain.
            chain[2].ain_pct ^= 0x01;
        }
    }
    Ok(())
}
