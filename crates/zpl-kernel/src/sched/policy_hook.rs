//! Kernel-side scheduler decision wrapper.
//!
//! Threshold values come from `crate::zpl_policy::PolicyConfig`, which
//! is whichever policy this build links:
//!
//! - `--features=engine`: configuration supplied by the decision engine,
//!   which is not part of this repository.
//! - `--features=public`: the demo policy in `zpl_policy`, whose rules
//!   and values are documented in that module.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelDecision {
    Allow,
    Degrade,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionReason {
    HighStability,
    MidStability,
    LowStability,
}

#[derive(Debug, Clone, Copy)]
pub struct PostBinaryScheduler {
    pub allow_threshold: f32,
    pub degrade_threshold: f32,
}

impl PostBinaryScheduler {
    pub const fn new() -> Self {
        let cfg = crate::zpl_policy::PolicyConfig::pb_core_default();
        Self {
            allow_threshold: cfg.allow_threshold as f32,
            degrade_threshold: cfg.degrade_threshold as f32,
        }
    }

    pub fn decide(&self, score: f32) -> KernelDecision {
        self.decide_with_reason(score).0
    }

    pub fn decide_with_reason(&self, score: f32) -> (KernelDecision, DecisionReason) {
        if score >= self.allow_threshold {
            (KernelDecision::Allow, DecisionReason::HighStability)
        } else if score >= self.degrade_threshold {
            (KernelDecision::Degrade, DecisionReason::MidStability)
        } else {
            (KernelDecision::Block, DecisionReason::LowStability)
        }
    }
}

impl Default for PostBinaryScheduler {
    fn default() -> Self {
        Self::new()
    }
}
