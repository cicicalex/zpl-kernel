//! Programs the kernel runs to show the gate working. Not part of the kernel proper.

// Three ring-3 programs, one per rule of the demo policy. Same configuration as
// the panel they are meant to be watched on.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    feature = "qemu_boot",
    feature = "vga_crit_mirror"
))]
pub mod demo_programs;

#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub mod ring3_demo;

// Also built for `user_programs`, which takes the ELF images and the ring-3 entry
// from here without the boot branch that runs one automatically.
#[cfg(all(
    target_os = "none",
    target_arch = "x86_64",
    any(feature = "elf_demo", feature = "user_programs")
))]
pub mod elf_demo;

/// Entering a ring-3 program from the prompt and coming back when it exits.
///
/// Bare-metal only, and only where the ELF loader is built.
pub mod user_run;

