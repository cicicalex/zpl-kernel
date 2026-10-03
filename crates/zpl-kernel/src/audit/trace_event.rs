//! Structured trace event emission helpers (`v2` format).
//!
//! See `docs/TRACE_EVENT_FORMAT.md` for the format specification. This
//! module provides a minimal, no-allocation emitter that writes one
//! line per event as a single `boot::emit_critical_marker`,
//! so it reaches the screen mirror as well as COM1.
//!
//! Legacy `[ZPL-XXX] body` markers are NOT routed through this module;
//! they continue to be emitted as inline byte slices to preserve the
//! existing QEMU smoke + determinism fixtures.


/// One key=value attribute in a structured event line.
#[derive(Clone, Copy)]
pub struct EventAttr<'a> {
    pub key: &'a str,
    pub value: &'a str,
}

/// Emit a `v2` structured event:
/// `[ZPL-EVT2 tag=<tag> t=<t> <attrs>...]\n`.
///
/// `t` is a boot-relative timestamp; pass `0` when no clock is available.
/// `attrs` keys must follow the format spec (lower-case alnum + `_`).
pub fn emit_event(tag: &str, t: u64, attrs: &[EventAttr<'_>]) {
    // Assembled in one buffer and sent as a single marker. Piece by piece it went
    // straight to COM1, which is why the build identity — the first thing a reader
    // wants to know about a boot — never showed up on the screen.
    let mut line = crate::drivers::console::MarkerLine::new();
    line.push(b"[ZPL-EVT2 tag=");
    line.push(tag.as_bytes());
    line.push(b" t=");
    line.push_dec(t);
    for attr in attrs.iter() {
        line.push(b" ");
        line.push(attr.key.as_bytes());
        line.push(b"=");
        line.push(attr.value.as_bytes());
    }
    line.push(b"]\n");
    crate::drivers::console::emit_critical_marker(line.as_slice());
}
