//! A fixed-size ring of what happened, for tests to read back.
//!
//! Separate from the boot log in `serial`: that one is for a person watching a
//! screen, this one is for an assertion. Nothing here allocates, and the ring
//! overwrites rather than growing.
#[derive(Debug, Clone, Copy)]
pub struct TraceEvent {
    pub task_id: u64,
    pub score: f32,
    pub decision: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct TraceRing<const N: usize> {
    pub data: [Option<TraceEvent>; N],
    pub head: usize,
    pub len: usize,
}

impl<const N: usize> TraceRing<N> {
    pub const fn new() -> Self {
        Self {
            data: [None; N],
            head: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, event: TraceEvent) {
        self.data[self.head] = Some(event);
        self.head = (self.head + 1) % N;
        self.len = self.len.saturating_add(1).min(N);
    }

    pub fn count(&self) -> usize {
        self.len
    }

    /// Copies events in chronological order (oldest -> newest) into `out`.
    /// Returns number of copied entries.
    pub fn snapshot_chronological(&self, out: &mut [TraceEvent]) -> usize {
        if self.len == 0 || out.is_empty() {
            return 0;
        }

        let available = self.len.min(N);
        let to_copy = available.min(out.len());
        let oldest = if self.len < N { 0 } else { self.head };
        let start = available.saturating_sub(to_copy);
        let mut written = 0usize;

        for i in 0..to_copy {
            let idx = (oldest + start + i) % N;
            if let Some(event) = self.data[idx] {
                out[written] = event;
                written += 1;
            }
        }

        written
    }
}

impl<const N: usize> Default for TraceRing<N> {
    fn default() -> Self {
        Self::new()
    }
}
