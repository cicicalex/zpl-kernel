//! The decision log, the trace format, and the boot timing.

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod audit_chain;

pub mod audit_bridge;

pub mod trace;

pub mod trace_event;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod timing;

