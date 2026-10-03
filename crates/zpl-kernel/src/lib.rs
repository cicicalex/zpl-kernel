//! A small x86-64 research kernel, `no_std`, that puts a policy gate in front of
//! every request that would change something.
//!
//! # What it does
//!
//! It boots -- under QEMU through Multiboot, and from an ISO through Limine on
//! real hardware -- sets up paging and its own allocators, separates ring 0 from
//! ring 3, and offers ten system calls. Along the way every state-changing
//! request goes through one gateway, which answers *before* the kernel acts:
//! allow, degrade, or block.
//!
//! `docs/ARCHITECTURE.md` follows a single request from ring 3 through the gate
//! and back, and is the place to start.
//!
//! # What it is not
//!
//! Not an operating system. Not a Linux competitor. There are no benchmarks here
//! comparing it to anything, nothing important should run on it, and the syscall
//! layer trusts the pointers ring 3 hands it -- there is no `copy_from_user` yet.
//! `docs/KNOWN_LIMITATIONS.md` is the full list, and it is long on purpose.
//!
//! # The policy engine is not in this crate
//!
//! [`zpl_policy`] is the gateway the kernel calls. Which implementation answers
//! it is a build feature:
//!
//! - `engine` links the private engine, which is not in this repository;
//! - `public` links a demo policy with three readable rules, so a kernel built
//!   from this source runs and the gate can be watched deciding. Those rules are
//!   a demonstration and not a measurement.
//!
//! Both present the same ABI, which is what lets one be swapped for the other
//! without a caller changing.
//!
//! # Finding your way around
//!
//! | area | modules |
//! |---|---|
//! | boot | [`start`], [`boot`], [`boot_stack`], [`loader_stage`] |
//! | memory | [`frame_alloc`], `heap`, `paging`, `stack_guard` |
//! | rings and calls | `gdt`, `tss`, `interrupts`, `syscall`, `elf` |
//! | storage and IPC | `ramfs`, `ipc`, `shm` |
//! | deciding | [`zpl_policy`], [`policy_hook`], [`scheduler`], `audit_chain` |
//! | saying so | [`serial`], [`trace`], [`trace_event`], `vga` |
//!
//! Much of the kernel is built for a bare-metal target and is compiled out on a
//! host, so `cargo doc` on a development machine shows fewer modules than a
//! kernel build contains. The names above that are not links are exactly
//! those: they exist in the kernel and not in these docs.

#![no_std]
#![cfg_attr(all(target_os = "none", target_arch = "x86_64"), feature(abi_x86_interrupt))]

extern crate alloc;

/// The kernel's version, in one place.
///
/// It is written on the screen twice -- once in the `BUILD-ID` marker and once
/// in the summary line at the bottom -- and for a while those were two separate
/// string literals. The bump to 0.5.0 moved one and not the other, so a machine
/// booted from a stick showed `version=v0.5.0` at the top and `ZPL kernel v0.4`
/// at the bottom, at the same time. Found on real hardware, from a photograph.
///
/// Both now read this.
pub const KERNEL_VERSION: &str = "v0.5.0";


#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "agent_debug_boot"
))]
pub mod agent_debug_com1;

pub mod boot;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod interrupts;
pub mod qemu_exit;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod hal;
pub mod drivers;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod matrix_ops;
// Not gated on `target_os = "none"`: the CPUID probe is plain user-mode
// x86 and compiles on the host, so its regression tests can actually run
// under `cargo test`. That is the point -- the rbx-clobber bug fixed on
// 26 September 2026 was invisible to every host test because this module
// did not exist on the host.
#[cfg(target_arch = "x86_64")]
pub mod smp;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod syscall;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod tss;
pub mod zpl_policy;
// The binding used by the `engine` build; see its own header.
#[cfg(feature = "engine")]
pub mod zpl_policy_engine;
pub mod ap_lifecycle;
pub mod ap_hooks;
pub mod arch;

/// Memory: physical frames, the heap, page tables, and the guards around the stack.
pub mod mm;
/// Choosing what runs, and the gate that decides whether it may.
pub mod sched;
/// Files in RAM, and the loader that turns one into a running program.
pub mod fs;
/// Talking between processes: pipes and shared memory.
pub mod ipc;
/// The decision log, the trace format, and the boot timing.
pub mod audit;
/// What a person sees and types: the command line, the panel, the v0.4 menu.
pub mod ui;
/// Programs the kernel runs to show the gate working. Not part of the kernel proper.
pub mod demo;
pub mod loader_stage;
/// Hardware diagnostic for a test image: what the machine does at boot, on one screen.
/// Only its decoders are compiled on a host, for their tests.
#[cfg(any(
    test,
    all(
        feature = "hw_diag",
        target_os = "none",
        target_arch = "x86_64",
        not(feature = "qemu_boot")
    )
))]
pub mod diag;
pub mod bridge;
pub mod kernel;
#[cfg(target_os = "none")]
pub mod panic;
pub mod start;
/// v0.5: behaviour translator — the scheduler input measured instead of hand-written.
#[cfg(feature = "v05_translator")]
pub mod behavior;

pub use bridge::{ExecutionAction, KernelExecutionBridge};
pub use ap_lifecycle::{ApLifecycleState, MAX_SECONDARY_BOOT_SOURCES};
pub use ap_hooks::{
    ApEventSource, ApHookAdapter, ApLifecycleEvent, NullApEventSource, VmApEventSource,
    DEFAULT_AP_EVENT_QUEUE_CAPACITY,
};
pub use crate::audit::audit_bridge::{KernelAuditExportBridge, UserspaceAuditFrame};
pub use kernel::{
    KernelState, KernelTaskQueue, KernelTraceRing, Task, TaskQueue, DEFAULT_QUEUE_CAPACITY,
    DEFAULT_TRACE_CAPACITY,
};
pub use crate::sched::policy_hook::{DecisionReason, KernelDecision, PostBinaryScheduler};
pub use crate::sched::scheduler::{SchedulerInput, SchedulerResult, StabilityScheduler};
pub use crate::audit::trace::{TraceEvent, TraceRing};
pub use arch::x86_64::{
    publish_external_boot_frame_v1, publish_minimal_external_boot_frame_v1, ExternalBootFrameV1,
};
pub use crate::mm::boot_stack::{BootStackKind, Multiboot2HandoffV1, Stivale2HandoffV1};
pub use loader_stage::{enter_from_external_runtime, ExternalBootRuntime, LoaderStageV1, PublishedHandoff};
