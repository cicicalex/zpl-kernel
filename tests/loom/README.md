# Loom test plan

This directory holds concurrency scenarios for the boot-log ring and the
atomic counters behind it.

- `boot_log_ring_model.rs` checks ordering and monotonicity on a reduced model.
- The tests run when `loom` is available, locally or in CI.
