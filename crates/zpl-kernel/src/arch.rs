//! Architecture-specific code, kept behind one module so the rest of the kernel
//! does not name a processor.
//!
//! There is one architecture today.
pub mod x86_64;

/// The one door to the x86 I/O ports. Everything that talks to a port goes
/// through here, so the safety argument is written once instead of eight times.
pub mod port;
