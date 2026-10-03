//! Audit-chain replay tool (partial scope).
//!
//! Reads a JSON-serialized audit chain (the same record layout as
//! `crates/zpl-kernel/src/audit_chain.rs::AuditRecord`) from a file
//! and re-verifies it end-to-end. Emits a PASS/FAIL summary plus, on
//! failure, the index of the first record that mismatched.
//!
//! Today the chain dump is produced manually (or by a test fixture);
//! once `#19` ships persistent on-disk audit storage and `#16` ships
//! ZPL-FS, this tool gains a `--device <disk.img>` flag that reads
//! the chain straight from the kernel-managed file.
//!
//! The hash here reproduces the kernel's own record mixer in
//! `crate::audit::audit_chain::record_hash`. That module is the single place that
//! describes it.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

const GENESIS_HASH: u64 = 0xDEAD_BEEF_CAFE_BABE;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

fn mix(state: u64) -> u64 {
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

#[derive(Parser, Debug)]
#[command(name = "audit-replay", about = "Replay & verify a ZPL audit chain JSON dump")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Verify an existing chain JSON file.
    Verify {
        chain: PathBuf,
    },
    /// Generate a deterministic clean + tampered fixture pair so the
    /// gate can run without a kernel-produced dump.
    GenFixture {
        /// Output directory for `chain-good.json` and
        /// `chain-tampered.json`.
        #[arg(long)]
        out: PathBuf,
        /// Number of records in the generated chain.
        #[arg(long, default_value_t = 5)]
        records: u64,
    },
    /// Persist an existing chain JSON into a ZPL-FS disk image
    /// (inode `AUDIT_CHAIN_INODE`).
    StoreFs {
        chain: PathBuf,
        image: PathBuf,
    },
    /// Read the audit chain from a ZPL-FS disk image and replay it.
    VerifyFs {
        image: PathBuf,
    },
    /// Walk an audit chain and print decision histogram + AIN
    /// distribution statistics.
    Summary {
        chain: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Verify { chain } => verify_file(chain),
        Cmd::GenFixture { out, records } => gen_fixture(out, records),
        Cmd::StoreFs { chain, image } => store_fs(chain, image),
        Cmd::VerifyFs { image } => verify_fs(image),
        Cmd::Summary { chain } => summary(chain),
    }
}

fn summary(path: PathBuf) -> Result<()> {
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("[E_REPLAY_050_READ] {}", path.display()))?;
    let records: Vec<AuditRecord> = serde_json::from_str(&raw).with_context(|| {
        format!("[E_REPLAY_051_PARSE] parsing {}", path.display())
    })?;
    if records.is_empty() {
        println!("audit-replay summary: empty chain");
        return Ok(());
    }

    let mut allow = 0u64;
    let mut degrade = 0u64;
    let mut block = 0u64;
    let mut other = 0u64;
    let mut ain_sum = 0u64;
    let mut ain_min: u8 = 100;
    let mut ain_max: u8 = 0;
    let mut ains: Vec<u8> = Vec::with_capacity(records.len());
    for r in records.iter() {
        match r.decision {
            0 => allow += 1,
            1 => degrade += 1,
            2 => block += 1,
            _ => other += 1,
        }
        ain_sum += r.ain_pct as u64;
        if r.ain_pct < ain_min { ain_min = r.ain_pct; }
        if r.ain_pct > ain_max { ain_max = r.ain_pct; }
        ains.push(r.ain_pct);
    }
    ains.sort();
    let median = ains[ains.len() / 2];
    let ain_avg = (ain_sum as f64) / (records.len() as f64);

    println!("# audit-replay summary -- {}", path.display());
    println!();
    println!("| metric          | value     |");
    println!("|-----------------|-----------|");
    println!("| total records   | {:>9} |", records.len());
    println!("| ALLOW (d=0)     | {:>9} |", allow);
    println!("| DEGRADE (d=1)   | {:>9} |", degrade);
    println!("| BLOCK (d=2)     | {:>9} |", block);
    if other > 0 {
        println!("| other decisions | {:>9} |", other);
    }
    println!("| ain min         | {:>7} % |", ain_min);
    println!("| ain median      | {:>7} % |", median);
    println!("| ain avg         | {:>7.1} % |", ain_avg);
    println!("| ain max         | {:>7} % |", ain_max);
    println!();
    println!("seq range: {} .. {}", records.first().unwrap().seq, records.last().unwrap().seq);
    Ok(())
}

fn verify_file(path: PathBuf) -> Result<()> {
    let raw = fs::read_to_string(&path).with_context(|| {
        format!("[E_REPLAY_001_READ] reading {}", path.display())
    })?;
    let records: Vec<AuditRecord> = serde_json::from_str(&raw).with_context(|| {
        format!("[E_REPLAY_002_PARSE] parsing {}", path.display())
    })?;

    let mut prev = GENESIS_HASH;
    for (idx, record) in records.iter().enumerate() {
        if record.prev_hash != prev {
            bail!(
                "[E_REPLAY_010_PREV_HASH] record {} prev_hash mismatch (expected 0x{:016x}, got 0x{:016x})",
                idx,
                prev,
                record.prev_hash
            );
        }
        let expected = record_hash(&record.body_for_hash());
        if expected != record.self_hash {
            bail!(
                "[E_REPLAY_011_SELF_HASH] record {} self_hash mismatch (expected 0x{:016x}, got 0x{:016x})",
                idx,
                expected,
                record.self_hash
            );
        }
        prev = record.self_hash;
    }

    println!(
        "audit-replay: PASS - {} records verified ({})",
        records.len(),
        path.display()
    );
    Ok(())
}

fn store_fs(chain_path: PathBuf, image_path: PathBuf) -> Result<()> {
    let raw = fs::read_to_string(&chain_path).with_context(|| {
        format!("[E_REPLAY_030_READ_CHAIN] {}", chain_path.display())
    })?;
    let mut device = fs::read(&image_path).with_context(|| {
        format!("[E_REPLAY_031_READ_IMAGE] {}", image_path.display())
    })?;
    {
        let mut zfs = zpl_fs::ZplFs::mount(&mut device).map_err(|e| {
            anyhow::anyhow!("[E_REPLAY_032_MOUNT] {:?}", e)
        })?;
        // Mount must already have the audit-chain inode reserved
        // (see `zpl-mkfs init-audit`). Skip allocation if it's a no-op.
        if let Err(zpl_fs::Error::InodeAlreadyAllocated) =
            zfs.allocate_reserved(zpl_fs::AUDIT_CHAIN_INODE)
        {
            // already reserved; fine.
        }
        zfs.append(zpl_fs::AUDIT_CHAIN_INODE, raw.as_bytes())
            .map_err(|e| anyhow::anyhow!("[E_REPLAY_033_APPEND] {:?}", e))?;
    }
    fs::write(&image_path, &device)?;
    println!(
        "audit-replay store-fs: persisted {} bytes of chain JSON into {}",
        raw.len(),
        image_path.display()
    );
    Ok(())
}

fn verify_fs(image_path: PathBuf) -> Result<()> {
    let mut device = fs::read(&image_path).with_context(|| {
        format!("[E_REPLAY_040_READ_IMAGE] {}", image_path.display())
    })?;
    let zfs = zpl_fs::ZplFs::mount(&mut device).map_err(|e| {
        anyhow::anyhow!("[E_REPLAY_041_MOUNT] {:?}", e)
    })?;
    let size = zfs
        .file_size(zpl_fs::AUDIT_CHAIN_INODE)
        .map_err(|e| anyhow::anyhow!("[E_REPLAY_042_SIZE] {:?}", e))?
        as usize;
    let mut buf = vec![0u8; size];
    zfs.read_at(zpl_fs::AUDIT_CHAIN_INODE, 0, &mut buf)
        .map_err(|e| anyhow::anyhow!("[E_REPLAY_043_READ] {:?}", e))?;
    let raw = std::str::from_utf8(&buf)
        .with_context(|| "[E_REPLAY_044_UTF8] audit chain inode bytes are not UTF-8")?;
    let records: Vec<AuditRecord> = serde_json::from_str(raw)
        .with_context(|| "[E_REPLAY_045_PARSE] parsing chain bytes as JSON")?;

    let mut prev = GENESIS_HASH;
    for (idx, record) in records.iter().enumerate() {
        if record.prev_hash != prev {
            anyhow::bail!(
                "[E_REPLAY_046_PREV_HASH] record {} prev_hash mismatch (image: {})",
                idx,
                image_path.display()
            );
        }
        let expected = record_hash(&record.body_for_hash());
        if expected != record.self_hash {
            anyhow::bail!(
                "[E_REPLAY_047_SELF_HASH] record {} self_hash mismatch (image: {})",
                idx,
                image_path.display()
            );
        }
        prev = record.self_hash;
    }

    println!(
        "audit-replay verify-fs: PASS - {} records verified from {}",
        records.len(),
        image_path.display()
    );
    Ok(())
}

fn gen_fixture(out: PathBuf, records: u64) -> Result<()> {
    fs::create_dir_all(&out).with_context(|| {
        format!("[E_REPLAY_020_MKDIR] {}", out.display())
    })?;

    let mut chain: Vec<AuditRecord> = Vec::with_capacity(records as usize);
    let mut prev = GENESIS_HASH;
    for seq in 0..records {
        let mut rec = AuditRecord {
            seq,
            tick: seq * 11,
            tid: (seq % 3) as u32,
            ain_pct: ((seq * 13) % 100) as u8,
            decision: (seq % 3) as u8,
            prev_hash: prev,
            self_hash: 0,
        };
        rec.self_hash = record_hash(&rec.body_for_hash());
        prev = rec.self_hash;
        chain.push(rec);
    }
    let good_path = out.join("chain-good.json");
    fs::write(
        &good_path,
        serde_json::to_string_pretty(&chain)?,
    )?;
    println!("audit-replay gen-fixture: wrote {}", good_path.display());

    let mut tampered = chain.clone();
    if let Some(record) = tampered.get_mut(records.saturating_sub(2) as usize) {
        record.ain_pct ^= 0xFF;
    }
    let bad_path = out.join("chain-tampered.json");
    fs::write(
        &bad_path,
        serde_json::to_string_pretty(&tampered)?,
    )?;
    println!(
        "audit-replay gen-fixture: wrote {} ({} records, last-1 tampered)",
        bad_path.display(),
        records
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_record(seq: u64, tid: u32, ain_pct: u8, decision: u8, prev: u64) -> AuditRecord {
        let mut rec = AuditRecord {
            seq,
            tick: seq * 11,
            tid,
            ain_pct,
            decision,
            prev_hash: prev,
            self_hash: 0,
        };
        rec.self_hash = record_hash(&rec.body_for_hash());
        rec
    }

    #[test]
    fn replay_clean_chain() {
        let mut records = Vec::new();
        let mut prev = GENESIS_HASH;
        for seq in 0..3u64 {
            let r = fresh_record(seq, (seq % 3) as u32, 90, 0, prev);
            prev = r.self_hash;
            records.push(r);
        }
        // Manual replay matches reference logic.
        let mut p = GENESIS_HASH;
        for r in records.iter() {
            assert_eq!(r.prev_hash, p);
            assert_eq!(r.self_hash, record_hash(&r.body_for_hash()));
            p = r.self_hash;
        }
    }

    #[test]
    fn replay_detects_self_hash_drift() {
        let mut records = Vec::new();
        let mut prev = GENESIS_HASH;
        for seq in 0..3u64 {
            let r = fresh_record(seq, 0, 80, 0, prev);
            prev = r.self_hash;
            records.push(r);
        }
        records[1].ain_pct = 99;
        let mut p = GENESIS_HASH;
        let mut bad = false;
        for r in records.iter() {
            if r.prev_hash != p {
                bad = true;
                break;
            }
            if record_hash(&r.body_for_hash()) != r.self_hash {
                bad = true;
                break;
            }
            p = r.self_hash;
        }
        assert!(bad, "tampered chain should fail replay");
    }
}
