//! Choosing which task runs next, and asking the policy gate before it does.
//!
//! Each tick scores the runnable tasks, picks the highest, and passes that
//! choice through the gate, which answers allow, degrade or block. The decision
//! is made before the task runs, never after -- see `docs/ARCHITECTURE.md` for
//! the path a request takes.
use crate::audit::trace::{TraceEvent, TraceRing};
use crate::sched::policy_hook::{DecisionReason, KernelDecision, PostBinaryScheduler};

#[derive(Debug, Clone, Copy)]
pub struct SchedulerInput {
    pub task_id: u64,
    pub stability_score: f32,
    pub estimated_cost: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerResult {
    pub decision: KernelDecision,
    pub reason: DecisionReason,
    pub adjusted_quantum: u32,
}

pub struct StabilityScheduler<const N: usize> {
    pub policy: PostBinaryScheduler,
    pub trace: TraceRing<N>,
}

impl<const N: usize> StabilityScheduler<N> {
    pub const fn new(policy: PostBinaryScheduler) -> Self {
        Self {
            policy,
            trace: TraceRing::new(),
        }
    }

    pub fn schedule(&mut self, input: SchedulerInput) -> SchedulerResult {
        let (decision, reason) = self.policy.decide_with_reason(input.stability_score);
        let adjusted_quantum = match decision {
            KernelDecision::Allow => input.estimated_cost,
            KernelDecision::Degrade => input.estimated_cost / 2,
            KernelDecision::Block => 0,
        };

        self.trace.push(TraceEvent {
            task_id: input.task_id,
            score: input.stability_score,
            decision: match decision {
                KernelDecision::Allow => 0,
                KernelDecision::Degrade => 1,
                KernelDecision::Block => 2,
            },
        });

        SchedulerResult {
            decision,
            reason,
            adjusted_quantum,
        }
    }
}
