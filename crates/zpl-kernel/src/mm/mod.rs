//! Memory: physical frames, the heap, page tables, and the guards around the stack.

pub mod frame_alloc;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod heap;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod paging;

pub mod phys_hhdm;

pub mod memory;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod stack_guard;

pub mod boot_stack;

