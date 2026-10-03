//! Files in RAM, and the loader that turns one into a running program.

/// The in-RAM filesystem. Nothing in it touches a port or a page table, so it is
/// built on the host too and its tests run in a plain `cargo test`.
pub mod ramfs;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod elf;

