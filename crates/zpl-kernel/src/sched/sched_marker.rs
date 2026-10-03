//! The one place a gate decision becomes visible.
//!
//! **Why this module exists.** There used to be two emitters of `[ZPL-SCHED …]`, one in
//! the timer interrupt and one in `sys_compute`, and each had its own private copy of the
//! serial write — a loop straight onto port 0x3F8. That made three things true at once,
//! and all three were wrong:
//!
//! 1. The running summary never saw a decision. It watches the bytes passing through
//!    `boot::boot_probe_byte`, and these did not pass through it. So `N decisions` counted
//!    only the shared-memory self-check's two, the boot fingerprint never folded in a
//!    scheduler line, and `last ain` stayed on whatever the self-check left there.
//! 2. On the ISO the lines were not on screen at all. The private copies mirrored into the
//!    VGA text buffer, and that boot path runs in a graphics mode where the text buffer
//!    goes nowhere.
//! 3. The panel contradicted itself in front of the reader: its `sys_compute` row, which is
//!    fed by a direct call, showed one score, and the summary two lines below showed
//!    another. Both were describing the same boot.
//!
//! The fix is not a wider pipe, it is a single one. `record` is the only way a decision is
//! announced: it tells the panel and emits the line, from the same arguments, in the same
//! call. The two cannot drift because there is nothing left to drift between.
//!
//! The line is also built whole and emitted once, instead of in seven fragments. The old
//! code's own comment asked for that — the colour was computed per fragment, so `action=…`
//! was coloured and the rest of the line was not.

/// Longest line `format_line` can produce.
///
/// `[ZPL-SCHED tid=` 15 + tid 4 + ` ain=` 5 + 3 digits + ` action=` 8 + `DEGRADE` 7 +
/// `]\n` 2 = 44. Rounded up, and the formatter clamps rather than trusting the sum.
pub const LINE_MAX: usize = 64;

/// The text for each verdict, exactly as the serial log has always carried it.
#[must_use]
pub fn label(decision: crate::zpl_policy::Decision) -> &'static [u8] {
    match decision {
        crate::zpl_policy::Decision::Allow => b"ALLOW",
        crate::zpl_policy::Decision::Degrade => b"DEGRADE",
        crate::zpl_policy::Decision::Block => b"BLOCK",
    }
}

/// Write `[ZPL-SCHED tid=<tid> ain=<pct> action=<label>]\n` into `buf`, returning its
/// length.
///
/// Pure, so the bytes can be checked on the host against what the watcher parses back out
/// of them — which is the property the two displays depend on.
pub fn format_line(
    tid: &[u8],
    ain_pct: u8,
    decision: crate::zpl_policy::Decision,
    buf: &mut [u8; LINE_MAX],
) -> usize {
    let mut n = 0;
    let mut push = |bytes: &[u8], n: &mut usize| {
        for &b in bytes {
            if *n < LINE_MAX {
                buf[*n] = b;
                *n += 1;
            }
        }
    };
    push(b"[ZPL-SCHED tid=", &mut n);
    push(tid, &mut n);
    push(b" ain=", &mut n);
    let mut digits = [0u8; 3];
    let d = u8_to_dec3(ain_pct, &mut digits);
    push(&digits[..d], &mut n);
    push(b" action=", &mut n);
    push(label(decision), &mut n);
    push(b"]\n", &mut n);
    n
}

/// Decimal, no leading zeros, at most three digits.
fn u8_to_dec3(value: u8, buf: &mut [u8; 3]) -> usize {
    if value >= 100 {
        buf[0] = b'0' + value / 100;
        buf[1] = b'0' + (value / 10) % 10;
        buf[2] = b'0' + value % 10;
        3
    } else if value >= 10 {
        buf[0] = b'0' + value / 10;
        buf[1] = b'0' + value % 10;
        2
    } else {
        buf[0] = b'0' + value;
        1
    }
}

/// Announce one gate decision: panel row and serial line, from the same arguments.
///
/// `site` and `who` are what the panel shows; `tid` is what the serial line has always
/// carried (a single task letter from the scheduler, `user` from a ring-3 call).
#[cfg(all(target_os = "none", target_arch = "x86_64"))]
pub fn record(
    site: crate::ui::gate_panel::Site,
    who: crate::ui::gate_panel::Who,
    tid: &[u8],
    ain_pct: u8,
    decision: crate::zpl_policy::Decision,
) {
    crate::ui::gate_panel::note(site, who, ain_pct, decision);

    let mut buf = [0u8; LINE_MAX];
    let n = format_line(tid, ain_pct, decision, &mut buf);
    // `emit_critical_marker`, not a private serial loop: it writes COM1, feeds the running
    // summary, and mirrors to whichever console this boot path has. That is the whole
    // point of the module.
    crate::drivers::console::emit_critical_marker(&buf[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zpl_policy::Decision;

    #[test]
    fn line_is_byte_for_byte_what_the_log_always_had() {
        let mut buf = [0u8; LINE_MAX];
        let n = format_line(b"A", 97, Decision::Allow, &mut buf);
        assert_eq!(&buf[..n], b"[ZPL-SCHED tid=A ain=97 action=ALLOW]\n");

        let n = format_line(b"user", 6, Decision::Block, &mut buf);
        assert_eq!(&buf[..n], b"[ZPL-SCHED tid=user ain=6 action=BLOCK]\n");

        let n = format_line(b"C", 100, Decision::Degrade, &mut buf);
        assert_eq!(&buf[..n], b"[ZPL-SCHED tid=C ain=100 action=DEGRADE]\n");
    }

    /// The defect this module was written for: the panel row said 6 and the summary said 5.
    ///
    /// Both now come from one `record` call, so the check is that the line `record` emits
    /// parses back to exactly the numbers it was handed. If a future edit changes the
    /// wording of the marker, or the watcher's parser, this fails.
    #[test]
    fn what_the_watcher_reads_back_is_what_the_panel_was_told() {
        for (tid, ain, decision, action_byte) in [
            (b"A".as_slice(), 97u8, Decision::Allow, b'A'),
            (b"user".as_slice(), 6, Decision::Block, b'B'),
            (b"B".as_slice(), 50, Decision::Degrade, b'D'),
            (b"user".as_slice(), 0, Decision::Block, b'B'),
            (b"C".as_slice(), 100, Decision::Allow, b'A'),
        ] {
            let mut buf = [0u8; LINE_MAX];
            let n = format_line(tid, ain, decision, &mut buf);
            crate::ui::v04_status::reset_for_test();
            for &b in &buf[..n] {
                crate::ui::v04_status::feed_byte(b);
            }
            let (seen_ain, seen_action) = crate::ui::v04_status::last_decision()
                .expect("a decision line must register as a decision");
            assert_eq!(
                (seen_ain, seen_action),
                (u32::from(ain), action_byte),
                "the summary read back something other than what the panel was told, for {tid:?}"
            );
            assert_eq!(
                crate::ui::v04_status::decisions(),
                1,
                "a decision line must be counted exactly once"
            );
        }
    }

    /// Nobody may write the marker anywhere else again.
    ///
    /// This is the half the arithmetic above cannot cover: the two displays agreed before,
    /// too, for every decision that went through the shared path. What broke them was a
    /// second emitter with its own serial write. So the test reads the two files that used
    /// to have one and asserts the literal is gone from both.
    #[test]
    fn there_is_only_one_emitter() {
        for (name, source) in [
            ("interrupts.rs", include_str!("../interrupts.rs")),
            ("syscall.rs", include_str!("../syscall.rs")),
        ] {
            for (i, line) in source.lines().enumerate() {
                // Doc comments may name the marker; code may not build it.
                let code = line.trim_start();
                if code.starts_with("//") || code.starts_with("///") {
                    continue;
                }
                assert!(
                    !code.contains("[ZPL-SCHED"),
                    "{name}:{} builds the decision marker itself; it must call \
                     sched_marker::record so the panel and the summary cannot disagree",
                    i + 1
                );
            }
        }
    }
}
