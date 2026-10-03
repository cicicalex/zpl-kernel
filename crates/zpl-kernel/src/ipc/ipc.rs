//! Pipes + IPC ring buffers.
//!
//! v0.1 design:
//! - `MAX_PIPES` (= 8) static slots, each carrying a 256-byte ring buffer.
//! - `pipe()` allocates a slot and returns a `(read_fd, write_fd)` pair
//!   that point to the same slot (read-end vs write-end is signalled by
//!   the LSB so user space can pass either descriptor without leaking
//!   kernel state).
//! - `send(write_fd, bytes)` enqueues bytes; `recv(read_fd, buf)` drains
//!   them in FIFO order. Drops bytes when the buffer is full.
//!
//! Acceptance: boot self-check creates a pipe, sends `"ipc-roundtrip"`,
//! receives, asserts equality. The "two concurrent processes" wording in
//! the plan is exercised symbolically here (one execution context plays
//! both producer and consumer) until ring-3 `SYS_FORK` lands.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::sync::atomic::{AtomicBool, Ordering};

pub const MAX_PIPES: usize = 8;
pub const PIPE_CAPACITY: usize = 256;

#[repr(C)]
pub struct Pipe {
    used: bool,
    head: u16,
    tail: u16,
    len: u16,
    data: [u8; PIPE_CAPACITY],
}

impl Pipe {
    pub const fn new() -> Self {
        Self {
            used: false,
            head: 0,
            tail: 0,
            len: 0,
            data: [0; PIPE_CAPACITY],
        }
    }
}

#[allow(clippy::declare_interior_mutable_const)]
const PIPE_INIT: Pipe = Pipe::new();
static mut PIPES: [Pipe; MAX_PIPES] = [PIPE_INIT; MAX_PIPES];
static INIT_DONE: AtomicBool = AtomicBool::new(false);

/// Reset the pipe table.
///
/// # Safety
/// Caller must guarantee no other thread is touching the table.
pub unsafe fn init() {
    unsafe {
        let table = (&raw mut PIPES) as *mut Pipe;
        for i in 0..MAX_PIPES {
            *table.add(i) = Pipe::new();
        }
    }
    INIT_DONE.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    NotInitialized,
    NoSlot,
    BadFd,
    Closed,
}

fn check_init() -> Result<(), IpcError> {
    if INIT_DONE.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err(IpcError::NotInitialized)
    }
}

const READ_BIT: usize = 0;
const WRITE_BIT: usize = 1;

fn make_fd(slot: usize, end: usize) -> u64 {
    ((slot as u64) << 1) | (end as u64)
}

fn split_fd(fd: u64) -> (usize, usize) {
    let slot = (fd >> 1) as usize;
    let end = (fd & 1) as usize;
    (slot, end)
}

/// Allocate a pipe slot, return `(read_fd, write_fd)`.
pub fn pipe() -> Result<(u64, u64), IpcError> {
    check_init()?;
    unsafe {
        let table = (&raw mut PIPES) as *mut Pipe;
        for i in 0..MAX_PIPES {
            let slot = &mut *table.add(i);
            if !slot.used {
                slot.used = true;
                slot.head = 0;
                slot.tail = 0;
                slot.len = 0;
                return Ok((make_fd(i, READ_BIT), make_fd(i, WRITE_BIT)));
            }
        }
    }
    Err(IpcError::NoSlot)
}

/// Enqueue bytes into the pipe behind `write_fd`. Returns the number of
/// bytes successfully written (may be < `buf.len()` if the buffer
/// fills).
pub fn send(write_fd: u64, buf: &[u8]) -> Result<usize, IpcError> {
    check_init()?;
    let (slot_idx, end) = split_fd(write_fd);
    if end != WRITE_BIT {
        return Err(IpcError::BadFd);
    }
    if slot_idx >= MAX_PIPES {
        return Err(IpcError::BadFd);
    }
    unsafe {
        let table = (&raw mut PIPES) as *mut Pipe;
        let slot = &mut *table.add(slot_idx);
        if !slot.used {
            return Err(IpcError::Closed);
        }
        let mut written = 0usize;
        for byte in buf.iter() {
            if slot.len as usize >= PIPE_CAPACITY {
                break;
            }
            slot.data[slot.tail as usize] = *byte;
            slot.tail = ((slot.tail as usize + 1) % PIPE_CAPACITY) as u16;
            slot.len += 1;
            written += 1;
        }
        Ok(written)
    }
}

/// Drain up to `buf.len()` bytes from the pipe behind `read_fd`.
pub fn recv(read_fd: u64, buf: &mut [u8]) -> Result<usize, IpcError> {
    check_init()?;
    let (slot_idx, end) = split_fd(read_fd);
    if end != READ_BIT {
        return Err(IpcError::BadFd);
    }
    if slot_idx >= MAX_PIPES {
        return Err(IpcError::BadFd);
    }
    unsafe {
        let table = (&raw mut PIPES) as *mut Pipe;
        let slot = &mut *table.add(slot_idx);
        if !slot.used {
            return Err(IpcError::Closed);
        }
        let mut read_n = 0usize;
        for out in buf.iter_mut() {
            if slot.len == 0 {
                break;
            }
            *out = slot.data[slot.head as usize];
            slot.head = ((slot.head as usize + 1) % PIPE_CAPACITY) as u16;
            slot.len -= 1;
            read_n += 1;
        }
        Ok(read_n)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    PipeFailed,
    SendFailed,
    RecvFailed,
    Mismatch,
}

const SELF_CHECK_PAYLOAD: &[u8] = b"ipc-roundtrip";

pub fn self_check() -> Result<(), SelfCheckErr> {
    let (rd, wr) = pipe().map_err(|_| SelfCheckErr::PipeFailed)?;
    let n = send(wr, SELF_CHECK_PAYLOAD).map_err(|_| SelfCheckErr::SendFailed)?;
    if n != SELF_CHECK_PAYLOAD.len() {
        return Err(SelfCheckErr::SendFailed);
    }
    let mut buf = [0u8; 32];
    let read_n = recv(rd, &mut buf).map_err(|_| SelfCheckErr::RecvFailed)?;
    if read_n != SELF_CHECK_PAYLOAD.len() {
        return Err(SelfCheckErr::Mismatch);
    }
    if &buf[..read_n] != SELF_CHECK_PAYLOAD {
        return Err(SelfCheckErr::Mismatch);
    }
    Ok(())
}
