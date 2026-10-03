//! Integration test runner: boots the ZPL kernel under QEMU, parses the
//! serial log, and asserts the expected `[ZPL-*]` markers for a named
//! scenario. Used by CI and by the local verification scripts.
//!
//! Usage:
//!   cargo run -p zpl-test-runner -- --scenario boot --expect-markers 3
//!
//! Exit codes:
//!   0  - all asserts passed
//!   1  - QEMU failed to launch / timed out
//!   2  - log missing required marker
//!   3  - unknown scenario

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "zpl-test-runner",
    about = "Boot the ZPL kernel under QEMU and assert log markers"
)]
struct Cli {
    /// Name of the scenario whose marker set to verify.
    #[arg(long, value_enum, default_value_t = Scenario::Boot)]
    scenario: Scenario,

    /// Lower bound for the count of `[ZPL-BOOT]` markers expected.
    #[arg(long, default_value_t = 3)]
    expect_markers: u32,

    /// Absolute or workspace-relative path to the kernel binary.
    /// Defaults to `target/x86_64-zpl-kernel/release/zpl-kernel-bin`.
    #[arg(long)]
    kernel: Option<PathBuf>,

    /// Maximum seconds to wait for QEMU to exit (or for the marker to be
    /// observed if QEMU stays alive on halt loop).
    #[arg(long, default_value_t = 20)]
    timeout_secs: u64,

    /// Override the path the QEMU process writes the serial log to.
    /// Defaults to `artifacts/kernel/test-run-<timestamp>.log`.
    #[arg(long)]
    log: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Scenario {
    /// Bootstrap path through to halt loop, including frame_alloc + paging
    /// self-checks and the BUILD-ID v2 trace event.
    Boot,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.scenario {
        Scenario::Boot => run_boot_scenario(cli),
    }
}

fn run_boot_scenario(cli: Cli) -> Result<()> {
    let kernel = cli
        .kernel
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/x86_64-zpl-kernel/release/zpl-kernel-bin"));
    if !kernel.exists() {
        bail!(
            "[E_RUNNER_001_KERNEL_MISSING] kernel binary not found: {}",
            kernel.display()
        );
    }

    let artifact_dir = Path::new("artifacts/kernel");
    if !artifact_dir.exists() {
        std::fs::create_dir_all(artifact_dir).context("creating artifacts/kernel")?;
    }
    let log_path = cli.log.clone().unwrap_or_else(|| {
        let ts = chrono_like_timestamp();
        artifact_dir.join(format!("test-run-{}.log", ts))
    });

    let timeout = Duration::from_secs(cli.timeout_secs);
    let qemu = which("qemu-system-x86_64")
        .context("[E_RUNNER_002_QEMU_PATH] qemu-system-x86_64 not on PATH")?;

    let mut cmd = Command::new(&qemu);
    cmd.args([
        "-kernel",
        kernel.to_str().unwrap(),
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0",
        "-serial",
        &format!("file:{}", log_path.display()),
        "-display",
        "none",
        "-no-reboot",
        "-no-shutdown",
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
        "-m",
        "256",
    ]);

    let mut child = cmd
        .spawn()
        .context("[E_RUNNER_003_QEMU_SPAWN] failed to spawn qemu-system-x86_64")?;

    let started = Instant::now();
    while started.elapsed() < timeout {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => bail!("[E_RUNNER_004_QEMU_WAIT] {}", e),
        }
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    std::thread::sleep(Duration::from_millis(500));

    let log = std::fs::read_to_string(&log_path)
        .with_context(|| format!("[E_RUNNER_005_LOG_READ] reading {}", log_path.display()))?;

    let boot_count = check_boot_log(&log, cli.expect_markers)
        .with_context(|| format!("[E_RUNNER_006_LOG] {}", log_path.display()))?;

    println!(
        "zpl-test-runner: scenario=boot PASS markers={} log={}",
        boot_count,
        log_path.display()
    );
    Ok(())
}

/// Decide whether a boot log says the boot worked.
///
/// Split out of the scenario so it can be tested: everything above it starts a virtual
/// machine and waits, and this is the part that decides PASS or FAIL. A mistake here
/// reports a broken boot as a good one, which is the one kind of mistake a test runner
/// must not make.
///
/// Returns how many `[ZPL-BOOT]` markers were seen.
fn check_boot_log(log: &str, expect_markers: u32) -> Result<u32> {
    let required = [
        "[ZPL-BOOT] kernel_entry reached",
        "[ZPL-BOOT] serial initialized",
        "[ZPL-BOOT] halt loop entered",
        "[ZPL-EVT2 tag=BUILD-ID",
        "[ZPL-FRAME] stress=1000000 leak=0",
        "[ZPL-HEAP] selfcheck box+vec100 OK",
        "[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK",
        "[ZPL-SMP max_leaf=",
        "[ZPL-VNET] probe",
        "[ZPL-PERF phase=0 name=kernel_entry",
        "[ZPL-PERF phase=15 name=halt_loop",
    ];
    for marker in required {
        if !log.contains(marker) {
            bail!(
                "[E_RUNNER_006_MISSING_MARKER] missing {}",
                marker
            );
        }
    }

    let boot_count = log.matches("[ZPL-BOOT]").count() as u32;
    if boot_count < expect_markers {
        bail!(
            "[E_RUNNER_007_BOOT_COUNT] expected >= {} ZPL-BOOT markers, got {}",
            expect_markers,
            boot_count
        );
    }

    Ok(boot_count)
}

fn which(cmd: &str) -> Result<PathBuf> {
    let path_var = std::env::var_os("PATH").context("PATH unset")?;
    for dir in std::env::split_paths(&path_var) {
        for ext in &["", ".exe", ".cmd", ".bat"] {
            let candidate = dir.join(format!("{}{}", cmd, ext));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!("`{}` not found on PATH", cmd)
}

fn chrono_like_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A good log, assembled from the markers the runner requires. Short on purpose: a
    /// real boot log is two thousand lines, and what is being tested is the decision,
    /// not the parsing of noise.
    fn good_log() -> String {
        [
            "[ZPL-BOOT] kernel_entry reached",
            "[ZPL-BOOT] serial initialized",
            "[ZPL-EVT2 tag=BUILD-ID t=0 kind=kernel_image version=v0.5.0]",
            "[ZPL-FRAME] stress=1000000 leak=0",
            "[ZPL-HEAP] selfcheck box+vec100 OK",
            "[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK",
            "[ZPL-SMP max_leaf=13 max_logical=0 apic_id=0 OK]",
            "[ZPL-VNET] probe bus=0 no virtio-net-pci",
            "[ZPL-PERF phase=0 name=kernel_entry rdtsc=1 delta=0]",
            "[ZPL-PERF phase=15 name=halt_loop rdtsc=2 delta=1]",
            "[ZPL-BOOT] halt loop entered",
        ]
        .join("\n")
    }

    #[test]
    fn a_complete_log_passes_and_counts_its_boot_markers() {
        let n = check_boot_log(&good_log(), 3).expect("a complete log must pass");
        assert_eq!(n, 3, "three [ZPL-BOOT] lines in the fixture");
    }

    /// The failure that matters: a boot that stopped partway must not be reported as
    /// good. Each required marker is removed in turn, and each removal must be caught.
    #[test]
    fn removing_any_required_marker_is_caught() {
        let full = good_log();
        for line in full.lines() {
            let without: Vec<&str> = full.lines().filter(|l| *l != line).collect();
            let res = check_boot_log(&without.join("\n"), 1);
            assert!(
                res.is_err(),
                "dropping {line:?} left a log the runner still calls a pass"
            );
        }
    }

    #[test]
    fn too_few_boot_markers_is_a_failure_even_when_every_marker_is_present() {
        // The count is the second half of the check: a log can contain every required
        // line and still describe a boot that did not get as far as it should.
        assert!(check_boot_log(&good_log(), 4).is_err());
        assert!(check_boot_log(&good_log(), 3).is_ok());
    }

    #[test]
    fn an_empty_log_is_a_failure_rather_than_a_pass_with_zero_markers() {
        assert!(check_boot_log("", 0).is_err());
    }

    #[test]
    fn the_message_names_the_marker_that_was_missing() {
        // So someone reading CI output knows where the boot stopped without opening
        // the log.
        let without = good_log().replace("[ZPL-BOOT] halt loop entered", "");
        let err = check_boot_log(&without, 1).unwrap_err().to_string();
        assert!(err.contains("halt loop entered"), "message was: {err}");
    }
}
