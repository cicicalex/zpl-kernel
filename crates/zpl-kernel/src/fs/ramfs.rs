//! Tiny in-RAM filesystem.
//!
//! Layout:
//! - `MAX_FILES` (= 8) fixed slots, each carrying a 64-byte ASCII path
//!   plus a 1024-byte data buffer.
//! - File descriptors are slot indices (`0..MAX_FILES`).
//! - "Open" returns the existing slot for a known path or allocates a
//!   fresh one if the path is new.
//! - "Close" is a no-op in v1 (fds remain valid until `init`).
//!
//! Acceptance: `self_check()` exercises
//! `create -> write -> close -> reopen -> read -> verify equal`.
//! Wired into `SYS_OPEN` / `SYS_READ` / `SYS_WRITE` for ring-3 access.
//!
//! Out of scope for v1: directories, permissions, persistence beyond
//! kernel lifetime.

use core::sync::atomic::{AtomicBool, Ordering};

pub const MAX_FILES: usize = 8;
pub const MAX_PATH: usize = 64;
pub const MAX_DATA: usize = 1024;

#[repr(C)]
pub struct File {
    used: bool,
    path_len: u8,
    path: [u8; MAX_PATH],
    data: [u8; MAX_DATA],
    len: u16,
}

impl Default for File {
    fn default() -> Self {
        Self::new()
    }
}

impl File {
    /// A free slot. `const` because the table is a static array of them, which
    /// `Default` cannot build.
    pub const fn new() -> Self {
        Self {
            used: false,
            path_len: 0,
            path: [0; MAX_PATH],
            data: [0; MAX_DATA],
            len: 0,
        }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const FILE_INIT: File = File::new();
static mut FILES: [File; MAX_FILES] = [FILE_INIT; MAX_FILES];
static INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Reset the filesystem. Must run once before the first call to any
/// other function in this module.
///
/// # Safety
/// Caller must ensure no other thread is touching the file table.
pub unsafe fn init() {
    unsafe {
        let table = (&raw mut FILES) as *mut File;
        for i in 0..MAX_FILES {
            *table.add(i) = File::new();
        }
    }
    INIT_DONE.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotInitialized,
    PathTooLong,
    EmptyPath,
    NoSlots,
    BadFd,
    OutOfSpace,
}

fn check_init() -> Result<(), FsError> {
    if INIT_DONE.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(FsError::NotInitialized)
    }
}

fn slot_path(slot: &File) -> &[u8] {
    &slot.path[..slot.path_len as usize]
}

/// Open `path`; if it already exists, return its slot, otherwise
/// allocate one. Empty `path` is rejected.
pub fn open(path: &[u8]) -> Result<usize, FsError> {
    check_init()?;
    if path.is_empty() {
        return Err(FsError::EmptyPath);
    }
    if path.len() > MAX_PATH {
        return Err(FsError::PathTooLong);
    }

    unsafe {
        let table = (&raw mut FILES) as *mut File;
        for i in 0..MAX_FILES {
            let slot = &mut *table.add(i);
            if slot.used && slot_path(slot) == path {
                return Ok(i);
            }
        }
        for i in 0..MAX_FILES {
            let slot = &mut *table.add(i);
            if !slot.used {
                slot.used = true;
                slot.path_len = path.len() as u8;
                slot.path[..path.len()].copy_from_slice(path);
                slot.len = 0;
                return Ok(i);
            }
        }
    }
    Err(FsError::NoSlots)
}

/// Replace the file's contents with `buf` (truncating to MAX_DATA).
pub fn write(fd: usize, buf: &[u8]) -> Result<usize, FsError> {
    check_init()?;
    if fd >= MAX_FILES {
        return Err(FsError::BadFd);
    }
    if buf.len() > MAX_DATA {
        return Err(FsError::OutOfSpace);
    }
    unsafe {
        let table = (&raw mut FILES) as *mut File;
        let slot = &mut *table.add(fd);
        if !slot.used {
            return Err(FsError::BadFd);
        }
        slot.data[..buf.len()].copy_from_slice(buf);
        slot.len = buf.len() as u16;
        Ok(buf.len())
    }
}

/// Read up to `buf.len()` bytes from the file into `buf`.
pub fn read(fd: usize, buf: &mut [u8]) -> Result<usize, FsError> {
    check_init()?;
    if fd >= MAX_FILES {
        return Err(FsError::BadFd);
    }
    unsafe {
        let table = (&raw mut FILES) as *mut File;
        let slot = &mut *table.add(fd);
        if !slot.used {
            return Err(FsError::BadFd);
        }
        let n = core::cmp::min(buf.len(), slot.len as usize);
        buf[..n].copy_from_slice(&slot.data[..n]);
        Ok(n)
    }
}

/// Close is a no-op in v1; provided for API parity. Subsequent `open`
/// calls on the same path return the same slot.
pub fn close(fd: usize) -> Result<(), FsError> {
    check_init()?;
    if fd >= MAX_FILES {
        return Err(FsError::BadFd);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    OpenFailed,
    WriteFailed,
    CloseFailed,
    ReopenFailed,
    ReadFailed,
    Mismatch,
}

const SELF_CHECK_PATH: &[u8] = b"/tmp/foo";
const SELF_CHECK_DATA: &[u8] = b"hello-ramfs";

/// Acceptance flow: `create -> write -> close -> reopen -> read ->
/// verify equal`.
/// One entry of a directory listing: the path and how many bytes are in the file.
pub type Entry = ([u8; MAX_PATH], u8, u16);

/// Copy the occupied entries out of the table. Returns how many were written.
///
/// The filesystem is flat -- there are no directories -- so this is the whole of it.
/// Taken in one pass, so two rows a listing shows cannot come from two moments.
pub fn list(out: &mut [Entry]) -> usize {
    let mut n = 0;
    // SAFETY: a read of a table only the boot CPU writes, taken while nothing else
    // is creating or closing a file. Same access the rest of this module uses.
    unsafe {
        let table = (&raw mut FILES) as *mut File;
        for i in 0..MAX_FILES {
            if n == out.len() {
                break;
            }
            let f = &*table.add(i);
            if !f.used {
                continue;
            }
            out[n] = (f.path, f.path_len, f.len);
            n += 1;
        }
    }
    n
}

pub fn self_check() -> Result<(), SelfCheckErr> {
    let fd = open(SELF_CHECK_PATH).map_err(|_| SelfCheckErr::OpenFailed)?;
    let written = write(fd, SELF_CHECK_DATA).map_err(|_| SelfCheckErr::WriteFailed)?;
    if written != SELF_CHECK_DATA.len() {
        return Err(SelfCheckErr::WriteFailed);
    }
    close(fd).map_err(|_| SelfCheckErr::CloseFailed)?;

    let fd2 = open(SELF_CHECK_PATH).map_err(|_| SelfCheckErr::ReopenFailed)?;
    let mut buf = [0u8; 32];
    let read_n = read(fd2, &mut buf).map_err(|_| SelfCheckErr::ReadFailed)?;
    if read_n != SELF_CHECK_DATA.len() {
        return Err(SelfCheckErr::Mismatch);
    }
    if &buf[..read_n] != SELF_CHECK_DATA {
        return Err(SelfCheckErr::Mismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_reports_what_was_created_and_nothing_else() {
        // SAFETY: a host test, single-threaded in its use of the table.
        unsafe { init() };
        let a = open(b"/a.bin").expect("open a");
        write(a, b"12345").expect("write a");
        close(a).expect("close a");
        let b = open(b"/b.bin").expect("open b");
        close(b).expect("close b");

        let mut rows = [([0u8; MAX_PATH], 0u8, 0u16); MAX_FILES];
        let n = list(&mut rows);
        assert_eq!(n, 2, "two files created, the rest of the table is free");

        let names: [&[u8]; 2] = [
            &rows[0].0[..rows[0].1 as usize],
            &rows[1].0[..rows[1].1 as usize],
        ];
        assert!(names.contains(&b"/a.bin".as_slice()));
        assert!(names.contains(&b"/b.bin".as_slice()));
        let a_row = rows[..n].iter().find(|r| &r.0[..r.1 as usize] == b"/a.bin").unwrap();
        assert_eq!(a_row.2, 5, "the length is what was written");
    }

    #[test]
    fn list_stops_at_the_end_of_a_short_buffer() {
        // SAFETY: as above.
        unsafe { init() };
        for i in 0..4u8 {
            let path = [b'/', b'a' + i];
            let fd = open(&path).expect("open");
            close(fd).expect("close");
        }
        let mut rows = [([0u8; MAX_PATH], 0u8, 0u16); 2];
        assert_eq!(list(&mut rows), 2, "writes no more than the buffer holds");
    }
}
