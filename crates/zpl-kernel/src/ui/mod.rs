//! What a person sees and types: the command line, the panel, the v0.4 menu.

/// `zpl-sh`: the kernel's command line.
///
/// Declared unconditionally, unlike the v0.4 menu modules: nothing in it touches a
/// port or a page table, so its parser and its formatting are compiled -- and
/// tested -- on the host by a plain `cargo test`.
pub mod shell;

// The policy panel draws into the rows the console keeps out of the scrolling log --
// the text buffer on the `-kernel` path, the framebuffer on the Limine path, both
// when both mirrors are on. It used to be `qemu_boot` only, which is why a machine
// booted from a stick never showed it.
//
// Compiled on every bare-metal build, not only the ones with a surface: `sched_marker`
// names `Site` and `Who` in the signature every decision goes through, so the types have
// to exist even where nothing is drawn. Without a surface `put_at` is already a no-op,
// so such a build keeps counters nobody reads and draws nowhere -- a few bytes, and the
// alternative was a second set of enums meaning the same thing.
#[cfg(any(all(target_os = "none", target_arch = "x86_64"), test))]
pub mod gate_panel;

/// v0.4 demo: the three-key menu shown after the self-check sequence.
#[cfg(feature = "v04_menu")]
pub mod v04_menu;

/// v0.4 demo: running counters and FNV-1a hash over the deterministic markers.
///
/// Declared unconditionally, for the same reason `kbd` is: it is arithmetic over bytes,
/// with no hardware in it, and the thing worth testing is whether what it reads back out
/// of a marker is what the kernel put in. `sched_marker`'s tests do exactly that, and they
/// need this compiled by a plain `cargo test`.
pub mod v04_status;

