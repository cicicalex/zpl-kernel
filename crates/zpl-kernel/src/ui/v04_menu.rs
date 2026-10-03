//! The v0.4 on-screen demo: three keys, three visible answers.
//!
//! The automatic self-check sequence runs first and is left exactly as it was; this menu
//! only reacts to keys pressed afterwards, from inside the halt loop. Every line it emits
//! is prefixed `[ZPL-V04` so it can be excluded from the determinism hash — the boot
//! sequence must stay byte-identical whether or not anyone touches the keyboard.
//!
//! - `1` — a clean workload, which the kernel allows.
//! - `2` — the hostile workload from `attack-test`, which the kernel blocks.
//! - `3` — the audit chain: how many records, and whether the chain still verifies.
//!
//! Both verdicts come from the gate, in both builds: the menu asks and prints the answer,
//! so `1` is allowed and `2` is refused because the gate said so, not because this file
//! decided in advance.
//!
//! With the shell in the build — which is how the ISO is built — the shell owns the
//! keyboard and these three keys are never read, so the banner stays quiet and the same
//! three answers are `run clean`, `run hostile` and `audit` at the prompt.
//!
//! The block reason is stated in words only. No threshold is ever printed; the `ain=`
//! value is shown because the existing scheduler markers already show it.

use crate::drivers::console::emit_critical_marker;
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
use crate::drivers::kbd::MenuKey;

/// Same inputs the SHM self-check uses, so what the menu shows matches what the boot
/// sequence already proved on serial.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const CLEAN_PAYLOAD: &[u8] = b"clean";
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
const HOSTILE_PAYLOAD: &[u8] = b"hostile";

/// Append decimal digits of `n` into `buf` at `pos`; returns the new position.
fn push_u64(mut n: u64, buf: &mut [u8], start: usize) -> usize {
    if n == 0 {
        if start < buf.len() {
            buf[start] = b'0';
            return start + 1;
        }
        return start;
    }
    let begin = start;
    let mut pos = start;
    while n > 0 && pos < buf.len() {
        buf[pos] = b'0' + (n % 10) as u8;
        pos += 1;
        n /= 10;
    }
    let mut lo = begin;
    let mut hi = pos - 1;
    while lo < hi {
        buf.swap(lo, hi);
        lo += 1;
        hi -= 1;
    }
    pos
}

fn push_bytes(src: &[u8], buf: &mut [u8], start: usize) -> usize {
    let mut pos = start;
    for &b in src {
        if pos >= buf.len() {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }
    pos
}

/// Emit one `[ZPL-V04 …]` line on COM1 and, when the console is up, on screen.
fn emit(parts: &[&[u8]], number: Option<u64>, tail: &[u8]) {
    let mut buf = [0u8; 160];
    let mut pos = 0;
    for p in parts {
        pos = push_bytes(p, &mut buf, pos);
    }
    if let Some(n) = number {
        pos = push_u64(n, &mut buf, pos);
    }
    pos = push_bytes(tail, &mut buf, pos);
    if pos < buf.len() {
        buf[pos] = b'\n';
        pos += 1;
    }
    emit_critical_marker(&buf[..pos]);
}

/// Run one workload through the shared-memory gate and report what the kernel decided.
///
/// One function for both builds, deliberately.
///
/// `shm_write` reaches the gate through `zpl_policy`, which resolves to the engine under
/// `--features=engine` and to the demonstration policy under `--features=public`. Neither
/// name appears here and the menu does not know which one answered — it prints whatever
/// verdict came back.
///
/// There used to be a second copy of this function for the public build which printed
/// "every write is refused" without asking the gate at all. That was true while the
/// public build was a stub that refused everything, and a lie from the moment the
/// demonstration policy replaced it: both keys printed the same refusal, so the one
/// thing the menu exists to show — a gate that decides — was the one thing it could not.
///
/// `shm` and `audit_chain` only exist on the bare-metal target, so the functions that
/// touch them are gated that way. The formatting helpers above stay host-compilable so
/// their unit tests actually run.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn run_workload(key_label: &[u8], payload: &[u8], bias: f64) {
    use crate::ipc::shm::{shm_create, shm_write, WriteOutcome};

    let id = match shm_create(b"v04demo") {
        Ok(id) => id,
        Err(_) => {
            emit(&[b"[ZPL-V04 menu=", key_label, b" shm-create-failed]"], None, b"");
            return;
        }
    };
    match shm_write(id, 0, payload, bias) {
        Ok(WriteOutcome::Allow { ain_pct }) => emit(
            &[b"[ZPL-V04 menu=", key_label, b" ain="],
            Some(u64::from(ain_pct)),
            b" action=ALLOW] clean program: ran",
        ),
        Ok(WriteOutcome::Degrade { ain_pct }) => emit(
            &[b"[ZPL-V04 menu=", key_label, b" ain="],
            Some(u64::from(ain_pct)),
            b" action=DEGRADE] ran with reduced resources",
        ),
        Ok(WriteOutcome::Block { ain_pct }) => emit(
            &[b"[ZPL-V04 menu=", key_label, b" ain="],
            Some(u64::from(ain_pct)),
            // Reason in words only - never a threshold.
            b" action=BLOCK] strongly skewed input: write refused",
        ),
        Err(_) => emit(&[b"[ZPL-V04 menu=", key_label, b" shm-write-failed]"], None, b""),
    }
}

/// Show how long the audit chain is and whether it still verifies.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
fn show_audit() {
    let len = crate::audit::audit_chain::len() as u64;
    emit(&[b"[ZPL-V04 menu=3 audit_len="], Some(len), b"]");
    match crate::audit::audit_chain::verify_chain() {
        Ok(n) => emit(
            &[b"[ZPL-V04 menu=3 verify=ok records="],
            Some(n as u64),
            b"] audit chain intact",
        ),
        Err(_) => emit(
            &[b"[ZPL-V04 menu=3 verify=FAIL]"],
            None,
            b" audit chain broken",
        ),
    }
}

/// Print the three choices once, under the self-check output.
///
/// Silent when the shell is in the build, which is the case on the ISO. Two readers of
/// one PS/2 controller would each swallow half the other's scancodes, so the menu's
/// three-key reader stands down for the prompt (see where `handle` is called). The keys
/// are then never read, and a banner advertising them is a promise the build does not
/// keep: pressing `1` types a `1` at the prompt. The same three answers are `run clean`,
/// `run hostile` and `audit`, which `help` lists.
///
/// The condition is written here, next to the reason, rather than at the call site, and
/// `banner_is_silent_exactly_when_the_keys_are_unread` checks the two still agree.
pub fn show_banner() {
    #[cfg(not(feature = "shell"))]
    emit(&[b"[ZPL-V04 menu] press 1 = clean program, 2 = hostile program, 3 = audit"], None, b"");
}

/// Act on one key press.
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn handle(key: MenuKey) {
    match key {
        MenuKey::One => run_workload(b"1", CLEAN_PAYLOAD, 0.5),
        MenuKey::Two => run_workload(b"2", HOSTILE_PAYLOAD, 0.95),
        MenuKey::Three => show_audit(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The banner advertises three keys. Something has to read them, and on the ISO the
    /// shell does instead, so the banner is silenced under `shell`. That condition lives
    /// in `show_banner`; the reader's lives at its call site in `boot.rs`. Two conditions
    /// that have to agree and sit in different files is exactly how an ISO ended up
    /// printing a banner for keys nothing was listening to, so this reads the other file
    /// and checks.
    #[test]
    fn banner_is_silent_exactly_when_the_keys_are_unread() {
        let boot = include_str!("../boot.rs");
        let reader = r#"#[cfg(all(feature = "v04_menu", not(feature = "shell")))]"#;
        assert!(
            boot.contains(reader),
            "boot.rs no longer gates the menu key reader on \
             `all(feature = \"v04_menu\", not(feature = \"shell\"))`. If the shell no \
             longer takes the keyboard, `show_banner` must stop silencing itself under \
             `shell`; if the condition merely moved, update this test with it."
        );
        let here = include_str!("v04_menu.rs");
        assert!(
            here.contains("#[cfg(not(feature = \"shell\"))]"),
            "show_banner no longer silences itself under `shell`, so a build with the \
             shell would advertise three keys that nothing reads."
        );
    }

    /// The public build must reach the gate, not answer for it. One `run_workload`, no
    /// `cfg` on the engine feature, and no trace of the refuse-everything stub that used
    /// to stand in for it.
    ///
    /// The patterns are anchored so the test does not match its own source: a check that
    /// reads the file it lives in will otherwise count its own string literals, which is
    /// how this test failed the first time it ran.
    #[test]
    fn the_menu_asks_the_gate_rather_than_answering_for_it() {
        let here = include_str!("v04_menu.rs");
        assert_eq!(
            here.matches("\nfn run_workload(").count(),
            1,
            "there must be exactly one run_workload: a second copy per build is how the \
             public menu came to print a verdict it never asked for"
        );
        // Split so the needle does not exist contiguously in this file: written whole, it
        // is itself the thing it searches for, and the test fails on its own source.
        let stub_marker = concat!("public", "-stub:");
        assert!(
            !here.contains(stub_marker),
            "the refuse-everything stub is back in the menu"
        );
        assert!(
            here.contains("shm_write(id, 0, payload, bias)"),
            "run_workload no longer puts the payload through the shared-memory gate"
        );
    }

    #[test]
    fn push_u64_writes_decimal() {
        let mut buf = [0u8; 8];
        let end = push_u64(0, &mut buf, 0);
        assert_eq!(&buf[..end], b"0");
        let mut buf = [0u8; 8];
        let end = push_u64(1234, &mut buf, 0);
        assert_eq!(&buf[..end], b"1234");
    }

    #[test]
    fn push_u64_does_not_overflow_its_buffer() {
        let mut buf = [0u8; 2];
        let end = push_u64(999_999, &mut buf, 0);
        assert!(end <= buf.len());
    }

    #[test]
    fn push_bytes_truncates_instead_of_panicking() {
        let mut buf = [0u8; 4];
        let end = push_bytes(b"abcdefgh", &mut buf, 0);
        assert_eq!(end, 4);
        assert_eq!(&buf[..end], b"abcd");
    }
}
