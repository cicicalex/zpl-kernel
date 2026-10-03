//! v0.4 running summary: what the kernel has decided so far, shown on screen.
//!
//! Every byte the kernel writes to COM1 passes through `boot::boot_probe_byte`, so that is
//! where lines are observed. Only the lines the determinism method keeps are folded into
//! the hash: they must start with `[ZPL-` and must not contain `ALIVE`, `rdtsc`, `PERF` or
//! `ZPL-V04`. That is the same filter as
//!
//! ```text
//! grep -a '^\[ZPL-' LOG | grep -a -v -e ALIVE -e rdtsc -e PERF -e ZPL-V04
//! ```
//!
//! so the value on screen can be recomputed from the serial log on any machine.
//!
//! The hash is **FNV-1a, 64-bit** — a published, ten-line algorithm, deliberately not the
//! mixer used by the audit chain.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Longest line we buffer. Anything longer is still hashed — it is folded in chunks —
/// but only this much is inspected for the counters.
const LINE_MAX: usize = 192;

static HASH: AtomicU64 = AtomicU64::new(FNV_OFFSET_BASIS);
static SELFCHECKS_OK: AtomicU32 = AtomicU32::new(0);
static DECISIONS: AtomicU32 = AtomicU32::new(0);
/// Last `ain=` value seen on a decision line, plus a marker byte for the action.
static LAST_AIN: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ACTION: AtomicU32 = AtomicU32::new(0);

/// A tiny single-writer line buffer. Bytes are folded in from one place only
/// (`boot_probe_byte`, with interrupts masked around whole markers), so a plain static
/// behind a raw pointer is enough and avoids pulling a lock into the serial path.
struct LineBuf {
    buf: [u8; LINE_MAX],
    len: usize,
}

impl LineBuf {
    const fn new() -> Self {
        Self {
            buf: [0; LINE_MAX],
            len: 0,
        }
    }
}

static mut LINE: LineBuf = LineBuf::new();

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn starts_with(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.len() >= prefix.len() && &haystack[..prefix.len()] == prefix
}

/// True when the determinism method would keep this line.
#[must_use]
pub fn line_counts(line: &[u8]) -> bool {
    starts_with(line, b"[ZPL-")
        && !contains(line, b"ALIVE")
        && !contains(line, b"rdtsc")
        && !contains(line, b"PERF")
        && !contains(line, b"ZPL-V04")
}

/// Fold one line (without its newline) into the running FNV-1a hash.
///
/// The trailing `\n` is folded too, so the result matches hashing the filtered log file
/// byte for byte.
fn fold(line: &[u8]) {
    let mut h = HASH.load(Ordering::Relaxed);
    for &b in line {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h ^= u64::from(b'\n');
    h = h.wrapping_mul(FNV_PRIME);
    HASH.store(h, Ordering::Relaxed);
}

/// Parse `ain=NN` out of a decision line.
fn parse_ain(line: &[u8]) -> Option<u32> {
    let needle = b"ain=";
    let mut i = 0;
    while i + needle.len() <= line.len() {
        if &line[i..i + needle.len()] == needle {
            let mut v: u32 = 0;
            let mut j = i + needle.len();
            let mut any = false;
            while j < line.len() && line[j].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add(u32::from(line[j] - b'0'));
                j += 1;
                any = true;
            }
            return if any { Some(v) } else { None };
        }
        i += 1;
    }
    None
}

fn note_line(line: &[u8]) {
    if !line_counts(line) {
        return;
    }
    fold(line);
    if contains(line, b" OK") {
        SELFCHECKS_OK.fetch_add(1, Ordering::Relaxed);
    }
    if contains(line, b"action=") {
        DECISIONS.fetch_add(1, Ordering::Relaxed);
        if let Some(ain) = parse_ain(line) {
            LAST_AIN.store(ain, Ordering::Relaxed);
        }
        let action = if contains(line, b"action=BLOCK") {
            b'B'
        } else if contains(line, b"action=DEGRADE") {
            b'D'
        } else {
            b'A'
        };
        LAST_ACTION.store(u32::from(action), Ordering::Relaxed);
    }
}

/// Observe one outgoing COM1 byte. Called from `boot::boot_probe_byte`.
pub fn feed_byte(b: u8) {
    // SAFETY: the kernel is single-threaded on this path and whole markers are emitted
    // with interrupts masked, so no other writer can interleave into this buffer.
    let line = unsafe { &mut *core::ptr::addr_of_mut!(LINE) };
    if b == b'\n' {
        let len = line.len;
        note_line(&line.buf[..len]);
        line.len = 0;
        return;
    }
    if b == b'\r' {
        return;
    }
    if line.len < LINE_MAX {
        line.buf[line.len] = b;
        line.len += 1;
    }
}

/// Running FNV-1a over the deterministic markers seen so far.
#[must_use]
pub fn hash() -> u64 {
    HASH.load(Ordering::Relaxed)
}

/// How many deterministic markers ended in `OK`.
#[must_use]
pub fn selfchecks_ok() -> u32 {
    SELFCHECKS_OK.load(Ordering::Relaxed)
}

/// How many scheduler / gate decisions have been seen.
#[must_use]
pub fn decisions() -> u32 {
    DECISIONS.load(Ordering::Relaxed)
}

/// Put every counter back where a fresh boot would have it.
///
/// Test-only, and only because these are process-wide statics: a test that feeds a marker
/// in and reads the counters back has to start from a known state, and `cargo test` runs
/// tests in one process. Nothing in the kernel calls it -- a boot starts at zero anyway.
#[cfg(test)]
pub fn reset_for_test() {
    HASH.store(FNV_OFFSET_BASIS, Ordering::Relaxed);
    SELFCHECKS_OK.store(0, Ordering::Relaxed);
    DECISIONS.store(0, Ordering::Relaxed);
    LAST_AIN.store(u32::MAX, Ordering::Relaxed);
    LAST_ACTION.store(0, Ordering::Relaxed);
    // SAFETY: single-threaded in the kernel; in tests the caller holds the only reference,
    // and the tests that use it do not run the line buffer concurrently.
    let line = unsafe { &mut *core::ptr::addr_of_mut!(LINE) };
    line.len = 0;
}

/// Last decision as `(ain, action_letter)`; `None` before the first one.
#[must_use]
pub fn last_decision() -> Option<(u32, u8)> {
    let ain = LAST_AIN.load(Ordering::Relaxed);
    if ain == u32::MAX {
        return None;
    }
    Some((ain, LAST_ACTION.load(Ordering::Relaxed) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_the_determinism_method() {
        assert!(line_counts(b"[ZPL-BOOT] kernel_entry reached"));
        assert!(line_counts(b"[ZPL-SCHED tid=B ain=97 action=ALLOW]"));
        assert!(!line_counts(b"[ZPL-ALIVE] tick=12"));
        assert!(!line_counts(b"[ZPL-PERF phase=0 name=kernel_entry]"));
        assert!(!line_counts(b"[ZPL-V04 menu=1 ain=99 action=ALLOW]"));
        assert!(!line_counts(b"{\"sessionId\":\"eadfc1\"}"));
    }

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        // Reference values for FNV-1a 64-bit.
        fn fnv(s: &[u8]) -> u64 {
            let mut h = FNV_OFFSET_BASIS;
            for &b in s {
                h ^= u64::from(b);
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        }
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn parses_ain_from_a_decision_line() {
        assert_eq!(parse_ain(b"[ZPL-SCHED tid=user ain=15 action=BLOCK]"), Some(15));
        assert_eq!(parse_ain(b"[ZPL-SCHED tid=B ain=97 action=ALLOW]"), Some(97));
        assert_eq!(parse_ain(b"[ZPL-BOOT] serial initialized"), None);
    }

    #[test]
    fn ain_parser_stops_at_the_buffer_end() {
        assert_eq!(parse_ain(b"ain="), None);
        assert_eq!(parse_ain(b"ain=7"), Some(7));
    }
}
