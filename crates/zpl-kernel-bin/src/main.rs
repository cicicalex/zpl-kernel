#![cfg_attr(all(target_os = "none", target_arch = "x86_64"), no_std)]
#![cfg_attr(all(target_os = "none", target_arch = "x86_64"), no_main)]

pub use zpl_kernel::start::*;

#[cfg(not(all(target_os = "none", target_arch = "x86_64")))]
fn main() {}
