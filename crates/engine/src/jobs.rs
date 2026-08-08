//! Two-layer job system: [`Scheduler`] trait (swappable backend) +
//! [`RayonScheduler`] (initial rayon-backed implementation).
//!
//! The trait defines the stable task API; the backend can be replaced
//! (e.g. with a custom crossbeam-deque scheduler) without changing callers.

use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender};

/// Opaque handle to a spawned task. Can be waited on or polled.
pub(crate) struct TaskHandle<T> {
    rx: Receiver<T>,
    cached: Mutex<Option<T>>,
}

impl<T: Send + 'static> TaskHandle<T> {
    fn new(rx: Receiver<T>) -> Self {
        Self {
            rx,
            cached: Mutex::new(None),
        }
    }

    /// Blocks until the task completes and returns the result.
    pub(crate) fn wait(self) -> T {
        let cached = self.cached.lock().expect("poisoned").take();
        if let Some(val) = cached {
            return val;
        }
        self.rx.recv().expect("task dropped without completing")
    }

    /// Returns true if the task has completed.
    pub(crate) fn is_complete(&self) -> bool {
        let mut cached = self.cached.lock().expect("poisoned");
        if cached.is_some() {
            return true;
        }
        match self.rx.try_recv() {
            Ok(val) => {
                *cached = Some(val);
                true
            }
            Err(_) => false,
        }
    }
}

/// Scheduler backend trait. Implementations provide the thread pool and
/// work distribution; callers use the stable API surface.
#[allow(dead_code)]
pub(crate) trait Scheduler: Send + Sync {
    /// Spawn a task on the pool. Returns a handle to wait on or poll.
    fn spawn<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> TaskHandle<T>;

    /// Parallel-for: split `range` into batches of `batch_size`, execute
    /// `f(index)` across pool workers. Blocks until all batches complete.
    fn parallel_for(
        &self,
        range: std::ops::Range<usize>,
        batch_size: usize,
        f: impl Fn(usize) + Send + Sync,
    );

    /// Number of worker threads in the pool.
    fn worker_count(&self) -> usize;
}

/// Rayon-backed scheduler. Work-stealing thread pool with parallel
/// iterators. Production-grade, zero custom scheduling code.
pub(crate) struct RayonScheduler {
    pool: rayon::ThreadPool,
}

impl RayonScheduler {
    /// Creates a scheduler with `count` worker threads.
    pub(crate) fn new(count: usize) -> Self {
        let count = count.max(2);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(count)
            .thread_name(|i| format!("sw-worker-{i}"))
            .build()
            .expect("failed to build rayon thread pool");

        log::info!("boot: rayon scheduler with {count} worker threads");

        Self { pool }
    }

    /// Creates a scheduler sized for the current hardware.
    /// Reserves 4 cores (main + GPU + audio + headroom), minimum 2 workers.
    pub(crate) fn auto() -> Self {
        let cores = thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4);
        let workers = cores.saturating_sub(4).max(2);
        Self::new(workers)
    }
}

impl Scheduler for RayonScheduler {
    fn spawn<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> TaskHandle<T> {
        let (tx, rx): (Sender<T>, Receiver<T>) = crossbeam_channel::bounded(1);
        self.pool.spawn(move || {
            let result = f();
            let _ = tx.send(result);
        });
        TaskHandle::new(rx)
    }

    fn parallel_for(
        &self,
        range: std::ops::Range<usize>,
        batch_size: usize,
        f: impl Fn(usize) + Send + Sync,
    ) {
        if range.is_empty() {
            return;
        }
        let batch_size = batch_size.max(1);
        let f = Arc::new(f);
        self.pool.install(|| {
            rayon::scope(|s| {
                let mut start = range.start;
                while start < range.end {
                    let end = (start + batch_size).min(range.end);
                    let f = Arc::clone(&f);
                    s.spawn(move |_| {
                        for i in start..end {
                            f(i);
                        }
                    });
                    start = end;
                }
            });
        });
    }

    fn worker_count(&self) -> usize {
        self.pool.current_num_threads()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn spawn_and_wait() {
        let sched = RayonScheduler::new(2);
        let handle = sched.spawn(|| 42u32);
        assert_eq!(handle.wait(), 42);
    }

    #[test]
    fn spawn_is_complete() {
        let sched = RayonScheduler::new(2);
        let handle = sched.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            99u32
        });
        let result = handle.wait();
        assert_eq!(result, 99);
    }

    #[test]
    fn is_complete_caches_value() {
        let sched = RayonScheduler::new(2);
        let handle = sched.spawn(|| 7u32);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(handle.is_complete());
        assert!(handle.is_complete());
        assert_eq!(handle.wait(), 7);
    }

    #[test]
    fn parallel_for_executes_all() {
        let sched = RayonScheduler::new(4);
        let count = Arc::new(AtomicUsize::new(0));
        let n = 1000;

        sched.parallel_for(0..n, 32, {
            let count = Arc::clone(&count);
            move |_| {
                count.fetch_add(1, Ordering::Relaxed);
            }
        });

        assert_eq!(count.load(Ordering::SeqCst), n);
    }

    #[test]
    fn parallel_for_empty_range() {
        let sched = RayonScheduler::new(2);
        let count = Arc::new(AtomicUsize::new(0));

        sched.parallel_for(0..0, 32, {
            let count = Arc::clone(&count);
            move |_| {
                count.fetch_add(1, Ordering::Relaxed);
            }
        });

        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn parallel_for_batch_size_one() {
        let sched = RayonScheduler::new(4);
        let sum = Arc::new(AtomicUsize::new(0));

        sched.parallel_for(0..100, 1, {
            let sum = Arc::clone(&sum);
            move |i| {
                sum.fetch_add(i, Ordering::Relaxed);
            }
        });

        assert_eq!(sum.load(Ordering::SeqCst), (0..100).sum::<usize>());
    }

    #[test]
    fn worker_count_matches() {
        let sched = RayonScheduler::new(3);
        assert_eq!(sched.worker_count(), 3);
    }

    #[test]
    fn auto_creates_at_least_two() {
        let sched = RayonScheduler::auto();
        assert!(sched.worker_count() >= 2);
    }

    #[test]
    fn fire_and_forget() {
        let sched = RayonScheduler::new(2);
        let flag = Arc::new(AtomicUsize::new(0));
        let flag2 = Arc::clone(&flag);

        let _handle = sched.spawn(move || {
            flag2.store(1, Ordering::SeqCst);
        });
        drop(_handle);

        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(flag.load(Ordering::SeqCst), 1);
    }
}
