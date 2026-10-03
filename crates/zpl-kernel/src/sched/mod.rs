//! Choosing what runs, and the gate that decides whether it may.

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod runqueue;

pub mod scheduler;

/// The process table. Nothing in it touches a port or a page table, so it is built
/// on the host too and its tests run in a plain `cargo test`.
pub mod process;

/// Announcing a gate decision: the panel row and the serial line, from one call.
pub mod sched_marker;

pub mod policy_hook;

