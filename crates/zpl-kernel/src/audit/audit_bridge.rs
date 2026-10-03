//! Turning a kernel decision record into the frame userspace tooling reads.
//!
//! The kernel's own record and the exported frame are deliberately separate
//! types: the first can change with the kernel, the second is what
//! `audit-replay` parses and therefore has to stay still.
use crate::audit::trace::TraceRing;

#[derive(Debug, Clone, Copy)]
pub struct UserspaceAuditFrame {
    pub task_id: u64,
    pub score: f32,
    pub decision: u8,
}

pub struct KernelAuditExportBridge;

impl KernelAuditExportBridge {
    pub const fn new() -> Self {
        Self
    }

    /// Copies kernel trace ring into a userspace-friendly flat frame buffer.
    /// Caller owns persistence/serialization.
    pub fn export_frames<const N: usize>(
        &self,
        ring: &TraceRing<N>,
        out: &mut [UserspaceAuditFrame],
    ) -> usize {
        if out.is_empty() {
            return 0;
        }

        let mut temp = [crate::audit::trace::TraceEvent {
            task_id: 0,
            score: 0.0,
            decision: 0,
        }; N];

        let copied = ring.snapshot_chronological(&mut temp);
        let to_write = copied.min(out.len());
        let start = copied.saturating_sub(to_write);

        for i in 0..to_write {
            let e = temp[start + i];
            out[i] = UserspaceAuditFrame {
                task_id: e.task_id,
                score: e.score,
                decision: e.decision,
            };
        }

        to_write
    }
}

impl Default for KernelAuditExportBridge {
    fn default() -> Self {
        Self::new()
    }
}
