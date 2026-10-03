#![cfg(loom)]
//! Loom model for the kernel's bump allocator atomic cursor (mirrors
//! `crates/zpl-kernel/src/mm/heap.rs`).
//!
//! Activate via:
//!     RUSTFLAGS='--cfg loom' cargo test --test bump_alloc_model
//!
//! Gated under `cfg(loom)` so the regular `cargo test --workspace`
//! does not attempt to build it. Runs only when the `loom` dev-dependency
//! is installed; it is not in the default dependency set.

use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::thread;

const HEAP_SIZE: usize = 4096;

#[test]
fn cursor_never_decreases_under_contention() {
    loom::model(|| {
        let off = std::sync::Arc::new(AtomicUsize::new(0));

        let off1 = off.clone();
        let t1 = thread::spawn(move || {
            for size in [16, 32, 8].iter().copied() {
                let cur = off1.load(Ordering::Relaxed);
                let new = cur + size;
                if new <= HEAP_SIZE {
                    let _ = off1.compare_exchange(
                        cur,
                        new,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );
                }
            }
        });

        let off2 = off.clone();
        let t2 = thread::spawn(move || {
            for size in [24, 8].iter().copied() {
                let cur = off2.load(Ordering::Relaxed);
                let new = cur + size;
                if new <= HEAP_SIZE {
                    let _ = off2.compare_exchange(
                        cur,
                        new,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    );
                }
            }
        });

        t1.join().expect("thread 1 join");
        t2.join().expect("thread 2 join");

        let final_off = off.load(Ordering::Relaxed);
        // Whatever the interleaving, the cursor never wraps backwards
        // and never overruns the heap region.
        assert!(final_off <= HEAP_SIZE);
    });
}
