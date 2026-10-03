//! A summary of what memory the kernel has and how much of it is in use.
//!
//! The counters here are reported, not enforced: the allocators in
//! `frame_alloc` and `heap` are what actually hand memory out.
#[derive(Debug, Clone, Copy)]
pub struct MemoryState {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

impl MemoryState {
    pub const fn new(total_bytes: u64) -> Self {
        Self {
            total_bytes,
            used_bytes: 0,
        }
    }

    pub fn reserve(&mut self, bytes: u64) {
        self.used_bytes = self.used_bytes.saturating_add(bytes).min(self.total_bytes);
    }

    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }
}
