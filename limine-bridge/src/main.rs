//! Limine bootloader → `zpl_boot_entry_v1` shim (single ELF on the USB ISO).
//!
//! On hosts (`target_os != "none"`) this crate builds a no-op `main` so
//! `cargo check --workspace` succeeds. The real Limine entry is `_start` in module `baremetal`.

#![cfg_attr(all(target_arch = "x86_64", target_os = "none"), no_std)]
#![cfg_attr(all(target_arch = "x86_64", target_os = "none"), no_main)]

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
mod baremetal;

/// The hardware diagnostic image's entry (`--features hw_diag`).
#[cfg(all(target_arch = "x86_64", target_os = "none", feature = "hw_diag"))]
mod diag;

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
fn main() {}
