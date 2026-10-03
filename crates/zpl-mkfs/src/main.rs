//! `zpl-mkfs` -- format a flat file as a ZPL-FS v0.1 image. Lets us produce a `disk.img` artifact that the kernel
//! virtio-blk wrapper and `audit-replay` can consume.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use zpl_fs::{format, ZplFs, AUDIT_CHAIN_INODE, BLOCK_SIZE, MAX_BLOCKS};

#[derive(Parser, Debug)]
#[command(name = "zpl-mkfs", about = "Format a file as a ZPL-FS v0.1 image")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Format an image file with an empty ZPL-FS.
    Format {
        path: PathBuf,
        /// Total size in 4 KiB blocks (must be in 4..=256).
        #[arg(long, default_value_t = 64)]
        blocks: usize,
    },
    /// Print stats from an existing image (`fsck`-style).
    Info {
        path: PathBuf,
    },
    /// Pre-allocate the audit-chain inode (reserved id 1).
    InitAudit {
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Format { path, blocks } => format_cmd(path, blocks),
        Cmd::Info { path } => info_cmd(path),
        Cmd::InitAudit { path } => init_audit_cmd(path),
    }
}

fn format_cmd(path: PathBuf, blocks: usize) -> Result<()> {
    if !(4..=MAX_BLOCKS).contains(&blocks) {
        bail!(
            "[E_MKFS_001_BLOCK_RANGE] blocks must be in 4..={} (got {})",
            MAX_BLOCKS,
            blocks
        );
    }
    let size = blocks * BLOCK_SIZE;
    let mut buf = vec![0u8; size];
    format(&mut buf).map_err(|e| anyhow::anyhow!("[E_MKFS_002_FORMAT] {:?}", e))?;
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("[E_MKFS_003_OPEN] {}", path.display()))?;
    f.write_all(&buf)?;
    f.seek(SeekFrom::Start(0))?;
    println!(
        "zpl-mkfs format: wrote {} blocks ({} bytes) to {}",
        blocks,
        size,
        path.display()
    );
    Ok(())
}

fn info_cmd(path: PathBuf) -> Result<()> {
    let mut buf = std::fs::read(&path)
        .with_context(|| format!("[E_MKFS_010_READ] {}", path.display()))?;
    let fs = ZplFs::mount(&mut buf)
        .map_err(|e| anyhow::anyhow!("[E_MKFS_011_MOUNT] {:?}", e))?;
    let report = fs
        .fsck()
        .map_err(|e| anyhow::anyhow!("[E_MKFS_012_FSCK] {:?}", e))?;
    println!("zpl-mkfs info: image {}", path.display());
    println!("  blocks total:  {}", report.block_count);
    println!("  files in use:  {}", report.inode_count);
    println!("  data blocks:   {}", report.data_blocks_used);
    println!("  bytes in use:  {}", report.bytes_total);
    Ok(())
}

fn init_audit_cmd(path: PathBuf) -> Result<()> {
    let mut buf = std::fs::read(&path)
        .with_context(|| format!("[E_MKFS_020_READ] {}", path.display()))?;
    {
        let mut fs = ZplFs::mount(&mut buf)
            .map_err(|e| anyhow::anyhow!("[E_MKFS_021_MOUNT] {:?}", e))?;
        fs.allocate_reserved(AUDIT_CHAIN_INODE)
            .map_err(|e| anyhow::anyhow!("[E_MKFS_022_ALLOC] {:?}", e))?;
    }
    std::fs::write(&path, &buf)?;
    println!(
        "zpl-mkfs init-audit: reserved audit-chain inode in {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This tool writes the disk image the kernel mounts. It had no tests: the
    /// crate had none, and an image written
    /// wrong does not fail here -- it fails inside a kernel, where nothing can report it.
    ///
    /// The checks are on the bounds and on the bytes, not on the printing: `format_cmd`
    /// writes a file, so each test writes into the OS temporary directory and removes it.
    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("zpl-mkfs-test-{name}-{}.img", std::process::id()));
        p
    }

    #[test]
    fn too_few_blocks_is_refused_rather_than_written() {
        // A three-block image has no room for the structures `format` lays down; writing
        // it would produce a file the kernel mounts and then reads past the end of.
        let p = tmp("tiny");
        assert!(format_cmd(p.clone(), 3).is_err());
        assert!(!p.exists(), "a refused format must not leave a file behind");
    }

    #[test]
    fn more_blocks_than_the_format_allows_is_refused() {
        let p = tmp("huge");
        assert!(format_cmd(p.clone(), MAX_BLOCKS + 1).is_err());
        assert!(!p.exists());
    }

    #[test]
    fn the_smallest_allowed_image_is_exactly_four_blocks_long() {
        let p = tmp("four");
        format_cmd(p.clone(), 4).expect("four blocks is the documented minimum");
        let n = std::fs::metadata(&p).expect("the file should exist").len();
        assert_eq!(n as usize, 4 * BLOCK_SIZE);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_formatted_image_is_one_zpl_fs_recognises() {
        // The point of the tool: what it writes, the filesystem reads back.
        let p = tmp("roundtrip");
        format_cmd(p.clone(), 64).expect("format");
        let mut buf = std::fs::read(&p).expect("read back");
        let fs = ZplFs::mount(&mut buf).expect("the image zpl-mkfs just wrote must mount");
        fs.fsck().expect("and it must pass its own consistency check");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn formatting_twice_replaces_the_image_rather_than_appending() {
        let p = tmp("twice");
        format_cmd(p.clone(), 64).expect("first");
        format_cmd(p.clone(), 8).expect("second");
        let n = std::fs::metadata(&p).expect("exists").len();
        assert_eq!(n as usize, 8 * BLOCK_SIZE, "the second format must truncate");
        std::fs::remove_file(&p).ok();
    }
}
