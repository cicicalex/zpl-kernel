//! The join between a scheduling decision and what the kernel then does about it.
//!
//! The scheduler answers with a decision; this turns that answer into an
//! `ExecutionAction` -- run it, run it with less, or do not run it -- and records
//! the outcome. Keeping the two apart is what lets the decision be tested
//! without a kernel and the action be tested without a policy.
use crate::audit::trace::TraceEvent;
use crate::kernel::{KernelState, Task};
use crate::sched::policy_hook::KernelDecision;
use crate::sched::scheduler::{SchedulerInput, StabilityScheduler};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionAction {
    Run { quantum: u32 },
    Defer { quantum: u32 },
    Drop,
}

/// Bridge between task metadata and post-binary scheduler decision.
/// Keeps kernel state synchronized with decision outcomes.
pub struct KernelExecutionBridge<const N: usize> {
    pub scheduler: StabilityScheduler<N>,
}

impl<const N: usize> KernelExecutionBridge<N> {
    pub const fn new(scheduler: StabilityScheduler<N>) -> Self {
        Self { scheduler }
    }

    pub fn evaluate_task(&mut self, state: &mut KernelState, task: Task) -> ExecutionAction {
        let input = SchedulerInput {
            task_id: task.id,
            stability_score: task.bias_hint,
            estimated_cost: task.priority as u32 * 10 + 10,
        };

        let result = self.scheduler.schedule(input);
        state.on_decision(result.decision);
        state.trace.push(TraceEvent {
            task_id: task.id,
            score: task.bias_hint,
            decision: map_decision_code(result.decision),
        });

        match result.decision {
            KernelDecision::Allow => ExecutionAction::Run {
                quantum: result.adjusted_quantum,
            },
            KernelDecision::Degrade => ExecutionAction::Defer {
                quantum: result.adjusted_quantum,
            },
            KernelDecision::Block => ExecutionAction::Drop,
        }
    }
}

fn map_decision_code(decision: KernelDecision) -> u8 {
    match decision {
        KernelDecision::Allow => 0,
        KernelDecision::Degrade => 1,
        KernelDecision::Block => 2,
    }
}
