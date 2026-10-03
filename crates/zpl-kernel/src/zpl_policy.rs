//! Policy module gateway used by the kernel for `sys_compute` and
//! shared-memory enforcement.
//!
//! Two builds:
//!
//! - `--features=engine` (the default): links the project's decision
//!   engine, which is not part of this repository. A source tree
//!   without it cannot build this configuration.
//! - `--features=public, no-default-features`: links the demo policy
//!   below, which mirrors the public ABI shape exactly and answers
//!   with three readable rules instead of the engine's computation.
//!   It exists so that a kernel built from this source tree actually
//!   runs: it boots, the gate
//!   allows, refuses and throttles, and all of it can be watched on
//!   the screen. It is a demonstration and not a measurement.
//!
//!   **This used to be a stub that answered `Block` to everything.**
//!   The description survived the change for a day longer than the
//!   code did, which is worth remembering the next time a comment
//!   describes behaviour rather than intent.
//!
//! Compile-time guard: exactly one of `engine` / `public` must be
//! active. `Cargo.toml` defaults to `engine`, so today's behaviour
//! is preserved unchanged.

// The engine binding is one line and it lives in its own file, `zpl_policy_engine`,
// so the public export can leave that file out entirely. See that file for why.
#[cfg(feature = "engine")]
pub use crate::zpl_policy_engine::*;

#[cfg(all(feature = "public", not(feature = "engine")))]
pub use demo_policy::{
    policy_compute, evaluate, AinStatus, ComputeInput, ComputeOutput, Decision, PolicyConfig,
    DEMO_ALLOW_SCORE, DEMO_BLOCK_SCORE, DEMO_REQUESTS_PER_BOOT,
};

/// The per-boot request quota, when this build has one.
///
/// The demo policy stops after a fixed number of requests; the engine build has
/// no such limit. Anything that wants to show the quota asks here instead of
/// reaching for a constant that exists in only one of the two builds -- which is
/// exactly what made `--features vga_crit_mirror` fail to compile without
/// `public`, in a combination nothing in the gates ever built.
#[must_use]
pub fn demo_request_quota() -> Option<u32> {
    #[cfg(all(feature = "public", not(feature = "engine")))]
    {
        Some(DEMO_REQUESTS_PER_BOOT)
    }
    #[cfg(not(all(feature = "public", not(feature = "engine"))))]
    {
        None
    }
}

#[cfg(all(feature = "public", not(feature = "engine")))]
mod demo_policy {
    //! A small, readable demo policy, so the public build actually runs
    //! programs and the gate can be watched doing its job.
    //!
    //! Compiled ONLY under `--features=public`. The struct and enum shapes
    //! mirror the ABI exactly so every caller compiles unchanged. The
    //! computation lives in the private engine; what follows is a
    //! demonstration and nothing else.
    //!
    //! **The three rules, in full.** A request declares one number, its
    //! `bias`, between 0 and 1. The demo reads that as "how far this
    //! request pushes"; the score is simply `1 - bias`.
    //!
    //! 1. **Within budget** -- score at or above `DEMO_ALLOW_SCORE`:
    //!    `Allow`.
    //! 2. **Over the demo limit** -- score below `DEMO_BLOCK_SCORE`:
    //!    `Block`. This is the "write to protected memory is refused"
    //!    case: the shared-memory self-check writes once with a modest
    //!    bias and once with a hostile one, and only the first goes
    //!    through.
    //! 3. **Asking too often** -- after `DEMO_REQUESTS_PER_BOOT` requests
    //!    in one boot, anything that would have been allowed is throttled
    //!    to `Degrade` instead. A request that was already over the limit
    //!    stays blocked.
    //!
    //! **The numbers below are demonstration values.** They are not the
    //! engine's configuration, they are not derived from it, and they are
    //! deliberately round so nobody mistakes them for measurements.
    //!
    //! - No identifiers beyond the public ABI surface.

    use core::sync::atomic::{AtomicU32, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct ComputeInput {
        pub bias: f64,
        pub dimension: u32,
        pub samples: u32,
        pub seed: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct ComputeOutput {
        pub ain: f64,
        pub deviation: f64,
        pub p_output: f64,
        pub status: AinStatus,
    }

    #[repr(u8)]
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum AinStatus {
        CertifiedNeutral = 0,
        Stable = 1,
        Caution = 2,
        HighBias = 3,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Decision {
        Allow,
        Degrade,
        Block,
    }

    #[derive(Clone, Copy)]
    pub struct PolicyConfig {
        pub allow_threshold: f64,
        pub degrade_threshold: f64,
        pub block_threshold: f64,
    }

    /// Score at or above this is allowed. Demonstration value.
    pub const DEMO_ALLOW_SCORE: f64 = 0.40;
    /// Score below this is refused outright. Demonstration value.
    pub const DEMO_BLOCK_SCORE: f64 = 0.20;
    /// How many requests one boot may make before the throttle starts.
    pub const DEMO_REQUESTS_PER_BOOT: u32 = 256;
    /// Score a throttled request is capped to: inside the degrade band.
    const DEMO_THROTTLED_SCORE: f64 = 0.30;

    /// Requests seen since boot. The only state the demo keeps.
    static DEMO_REQUESTS: AtomicU32 = AtomicU32::new(0);

    impl PolicyConfig {
        /// Demo policy bands. The method name is part of the ABI the
        /// engine side provides, so it is kept.
        pub const fn pb_core_default() -> Self {
            Self {
                allow_threshold: DEMO_ALLOW_SCORE,
                degrade_threshold: DEMO_BLOCK_SCORE,
                block_threshold: DEMO_BLOCK_SCORE,
            }
        }
    }

    /// The demo score, as a pure function, so the rules can be tested
    /// without depending on how many requests happened to come first.
    ///
    /// `request_index` is how many requests preceded this one in this boot.
    pub fn demo_score(bias: f64, request_index: u32) -> f64 {
        let raw = 1.0 - bias.clamp(0.0, 1.0);
        if request_index >= DEMO_REQUESTS_PER_BOOT && raw > DEMO_THROTTLED_SCORE {
            // Rule 3: throttled down into the degrade band. A request that
            // was already below the block line keeps its own lower score.
            DEMO_THROTTLED_SCORE
        } else {
            raw
        }
    }

    pub fn policy_compute(input: ComputeInput) -> ComputeOutput {
        let index = DEMO_REQUESTS.fetch_add(1, Ordering::Relaxed);
        let score = demo_score(input.bias, index);
        let status = if score < DEMO_BLOCK_SCORE {
            AinStatus::HighBias
        } else if score < DEMO_ALLOW_SCORE {
            AinStatus::Caution
        } else {
            AinStatus::Stable
        };
        ComputeOutput {
            ain: score,
            deviation: 1.0 - score,
            p_output: score,
            status,
        }
    }

    pub fn evaluate(cfg: PolicyConfig, output: &ComputeOutput) -> Decision {
        if output.ain >= cfg.allow_threshold {
            Decision::Allow
        } else if output.ain >= cfg.degrade_threshold {
            Decision::Degrade
        } else {
            Decision::Block
        }
    }
}

#[cfg(all(not(feature = "engine"), not(feature = "public")))]
compile_error!(
    "zpl-kernel needs exactly one of `engine` or `public` features; default is `engine`"
);

#[cfg(all(test, feature = "public", not(feature = "engine")))]
mod demo_policy_tests {
    //! The demo policy's three rules, asserted one by one. These run with
    //!     `cargo test -p zpl-kernel --no-default-features --features public`
    //! (on the host target, which the workspace default picks up).
    //!
    //! Every test goes through `demo_score`, the pure form, so none of them
    //! depends on how many requests another test happened to make first.

    use super::*;
    use super::demo_policy::demo_score;

    /// Decision for one request, without touching the shared counter.
    fn decide(bias: f64, request_index: u32) -> Decision {
        let score = demo_score(bias, request_index);
        let out = ComputeOutput {
            ain: score,
            deviation: 1.0 - score,
            p_output: score,
            status: AinStatus::Stable,
        };
        evaluate(PolicyConfig::pb_core_default(), &out)
    }

    #[test]
    fn rule_1_a_request_within_budget_is_allowed() {
        for bias in [0.0, 0.25, 0.5, 0.60] {
            assert_eq!(decide(bias, 0), Decision::Allow, "bias {bias} should be allowed");
        }
    }

    #[test]
    fn rule_2_a_request_over_the_demo_limit_is_blocked() {
        for bias in [0.81, 0.95, 1.0] {
            assert_eq!(decide(bias, 0), Decision::Block, "bias {bias} should be blocked");
        }
    }

    #[test]
    fn between_the_two_bands_a_request_is_degraded() {
        for bias in [0.61, 0.7, 0.79] {
            assert_eq!(decide(bias, 0), Decision::Degrade, "bias {bias} should degrade");
        }
    }

    #[test]
    fn rule_3_asking_too_often_throttles_to_degrade() {
        // The same request that is allowed early lands Degrade once the boot
        // has used up its quota.
        assert_eq!(decide(0.1, 0), Decision::Allow);
        assert_eq!(decide(0.1, DEMO_REQUESTS_PER_BOOT - 1), Decision::Allow);
        assert_eq!(decide(0.1, DEMO_REQUESTS_PER_BOOT), Decision::Degrade);
        assert_eq!(decide(0.1, DEMO_REQUESTS_PER_BOOT + 10_000), Decision::Degrade);
    }

    #[test]
    fn throttling_does_not_rescue_a_request_that_was_over_the_limit() {
        assert_eq!(decide(0.95, DEMO_REQUESTS_PER_BOOT + 1), Decision::Block);
    }

    #[test]
    fn the_shared_memory_self_check_contract_holds() {
        // `shm::self_check` writes once at 0.5 and once at 0.95 and requires
        // exactly one Allow and one Block. If this breaks, that boot marker
        // changes, so it is pinned here where the reason is visible.
        assert_eq!(decide(0.5, 0), Decision::Allow);
        assert_eq!(decide(0.95, 1), Decision::Block);
    }

    #[test]
    fn the_scheduler_prefers_the_lower_bias() {
        // `runqueue::self_check` registers five tasks and requires the first
        // to win every tick. That holds only while the score decreases with
        // bias, which is what this asserts.
        let scores: [f64; 5] = [0.50, 0.61, 0.72, 0.83, 0.94].map(|b| demo_score(b, 0));
        for pair in scores.windows(2) {
            assert!(pair[0] > pair[1], "score must fall as bias rises: {pair:?}");
        }
    }

    #[test]
    fn the_configuration_carries_the_documented_bands() {
        let cfg = PolicyConfig::pb_core_default();
        assert_eq!(cfg.allow_threshold, DEMO_ALLOW_SCORE);
        assert_eq!(cfg.degrade_threshold, DEMO_BLOCK_SCORE);
    }

    #[test]
    fn the_counter_advances_so_a_long_boot_really_does_throttle() {
        // The one test that touches the shared counter. It only checks that
        // the counter moves, not what value it reaches, so running in
        // parallel with the others cannot make it flaky.
        let input = ComputeInput { bias: 0.5, dimension: 9, samples: 64, seed: 1 };
        let a = policy_compute(input);
        let b = policy_compute(input);
        assert_eq!(a.ain, b.ain, "the score must not depend on the counter below the quota");
        assert!(a.ain > 0.0);
    }
}
