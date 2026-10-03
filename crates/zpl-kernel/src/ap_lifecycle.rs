//! Bookkeeping for secondary CPUs: which boot sources exist, and what state each
//! one is in.
//!
//! The kernel does not start a second core yet -- see `smp` for what it does
//! do -- so this is the record-keeping half of SMP without the bring-up half.
//! It is exercised by host tests rather than by a running second processor.
pub const MAX_SECONDARY_BOOT_SOURCES: usize = 8;
pub const MAX_SOURCE_IDS: u16 = 256;

#[derive(Debug, Clone, Copy)]
pub struct ApLifecycleState {
    primary_source: u16,
    secondary_sources: [u16; MAX_SECONDARY_BOOT_SOURCES],
    secondary_count: usize,
    online_mask: u32,
    rotation_cursor: usize,
}

impl ApLifecycleState {
    pub fn from_boot(primary_source: u16, cpu_count: u32) -> Self {
        let primary = primary_source % MAX_SOURCE_IDS;
        let mut secondary_sources = [0u16; MAX_SECONDARY_BOOT_SOURCES];
        let desired = cpu_count.saturating_sub(1) as usize;
        let secondary_count = desired.min(MAX_SECONDARY_BOOT_SOURCES);
        let mut i = 0usize;
        while i < secondary_count {
            secondary_sources[i] = primary.wrapping_add((i + 1) as u16) % MAX_SOURCE_IDS;
            i += 1;
        }
        Self {
            primary_source: primary,
            secondary_sources,
            secondary_count,
            online_mask: 0,
            rotation_cursor: 0,
        }
    }

    pub const fn primary_source(self) -> u16 {
        self.primary_source
    }

    pub const fn secondary_count(self) -> usize {
        self.secondary_count
    }

    pub const fn has_secondary_sources(self) -> bool {
        self.secondary_count > 0
    }

    pub fn bootstrap_mark_all_secondary_online<F>(&mut self, mut on_online: F)
    where
        F: FnMut(u16),
    {
        let mut idx = 0usize;
        while idx < self.secondary_count {
            self.set_online(idx, true);
            on_online(self.secondary_sources[idx]);
            idx += 1;
        }
    }

    pub fn register_ap_online<F>(&mut self, source: u16, mut on_online: F)
    where
        F: FnMut(u16),
    {
        if let Some(idx) = self.secondary_index(source) {
            if !self.is_online(idx) {
                self.set_online(idx, true);
                on_online(self.secondary_sources[idx]);
            }
        }
    }

    pub fn register_ap_offline<F>(&mut self, source: u16, mut on_offline: F)
    where
        F: FnMut(u16),
    {
        if let Some(idx) = self.secondary_index(source) {
            if self.is_online(idx) {
                self.set_online(idx, false);
                on_offline(self.secondary_sources[idx]);
            }
        }
    }

    pub fn register_ap_heartbeat<F>(&self, source: u16, mut on_heartbeat: F)
    where
        F: FnMut(u16),
    {
        if let Some(idx) = self.secondary_index(source) {
            if self.is_online(idx) {
                on_heartbeat(self.secondary_sources[idx]);
            }
        }
    }

    pub fn emit_heartbeat_for_online_sources<F>(&self, mut on_heartbeat: F)
    where
        F: FnMut(u16),
    {
        let mut idx = 0usize;
        while idx < self.secondary_count {
            if self.is_online(idx) {
                on_heartbeat(self.secondary_sources[idx]);
            }
            idx += 1;
        }
    }

    pub fn rotate_one_secondary_source<F>(&mut self, mut on_toggle: F)
    where
        F: FnMut(u16, bool),
    {
        if self.secondary_count == 0 {
            return;
        }
        let idx = self.rotation_cursor % self.secondary_count;
        let new_online = !self.is_online(idx);
        self.set_online(idx, new_online);
        on_toggle(self.secondary_sources[idx], new_online);
        self.rotation_cursor = (self.rotation_cursor + 1) % self.secondary_count;
    }

    fn set_online(&mut self, idx: usize, online: bool) {
        if idx >= self.secondary_count || idx >= 32 {
            return;
        }
        let bit = 1u32 << idx;
        if online {
            self.online_mask |= bit;
        } else {
            self.online_mask &= !bit;
        }
    }

    fn is_online(&self, idx: usize) -> bool {
        if idx >= self.secondary_count || idx >= 32 {
            return false;
        }
        let bit = 1u32 << idx;
        (self.online_mask & bit) != 0
    }

    fn secondary_index(&self, source: u16) -> Option<usize> {
        let source_norm = source % MAX_SOURCE_IDS;
        let mut idx = 0usize;
        while idx < self.secondary_count {
            if self.secondary_sources[idx] == source_norm {
                return Some(idx);
            }
            idx += 1;
        }
        None
    }
}
