//! Talking between processes: pipes and shared memory.

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod ipc;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod shm;

