#![cfg(loom)]

use loom::sync::atomic::{AtomicU32, Ordering};
use loom::thread;

#[test]
fn ring_seq_counter_is_monotonic_under_contention() {
    loom::model(|| {
        let seq = AtomicU32::new(1);
        let t1 = thread::spawn(|| {
            for _ in 0..32 {
                let _ = seq.fetch_add(1, Ordering::Relaxed);
            }
        });
        let t2 = thread::spawn(|| {
            for _ in 0..32 {
                let _ = seq.fetch_add(1, Ordering::Relaxed);
            }
        });
        t1.join().expect("thread 1 join");
        t2.join().expect("thread 2 join");
        let final_seq = seq.load(Ordering::Relaxed);
        assert!(final_seq >= 65);
    });
}
