//! Two-layer job system: [`Scheduler`] trait (swappable backend) +
//! [`RayonScheduler`] (initial rayon-backed implementation).
//!
//! The trait defines the stable task API; the backend can be replaced
//! (e.g. with a custom crossbeam-deque scheduler) without changing callers.
//!
//! Task dependencies use atomic prerequisite counting (UE5 pattern):
//! each pending task tracks an `AtomicU32` of outstanding prerequisites.
//! Predecessors decrement on completion; the last decrement spawns the task.
//! Fully decentralized — no global lock on the scheduling critical path.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender};

// ---------------------------------------------------------------------------
// Completion callback infrastructure
// ---------------------------------------------------------------------------

/// Shared, type-erased completion state for a task. Tracks whether the task
/// has finished and holds callbacks to fire on completion (used by the
/// dependency system to decrement prerequisite counters).
struct TaskInner {
    completed: AtomicBool,
    on_complete: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

impl TaskInner {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            on_complete: Mutex::new(Vec::new()),
        }
    }

    /// Mark complete and fire all registered callbacks.
    fn complete(&self) {
        self.completed.store(true, Ordering::Release);
        let callbacks: Vec<_> = self
            .on_complete
            .lock()
            .expect("poisoned")
            .drain(..)
            .collect();
        for cb in callbacks {
            cb();
        }
    }

    /// Register a callback. If already complete, fires immediately.
    fn on_complete(&self, cb: impl FnOnce() + Send + 'static) {
        let mut cbs = self.on_complete.lock().expect("poisoned");
        if self.completed.load(Ordering::Acquire) {
            drop(cbs);
            cb();
        } else {
            cbs.push(Box::new(cb));
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency
// ---------------------------------------------------------------------------

/// Type-erased dependency handle. Doesn't carry the result type — only
/// signals completion. Obtained from `TaskHandle::dependency()`.
#[allow(dead_code)]
pub(crate) struct Dependency {
    inner: Arc<TaskInner>,
}

// ---------------------------------------------------------------------------
// TaskHandle
// ---------------------------------------------------------------------------

/// Opaque handle to a spawned task. Can be waited on or polled.
pub(crate) struct TaskHandle<T> {
    rx: Receiver<T>,
    cached: Mutex<Option<T>>,
    inner: Arc<TaskInner>,
}

impl<T: Send + 'static> TaskHandle<T> {
    fn new(rx: Receiver<T>, inner: Arc<TaskInner>) -> Self {
        Self {
            rx,
            cached: Mutex::new(None),
            inner,
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

    /// Returns a type-erased dependency handle for use with `spawn_after`.
    #[allow(dead_code)]
    pub(crate) fn dependency(&self) -> Dependency {
        Dependency {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ---------------------------------------------------------------------------
// join
// ---------------------------------------------------------------------------

/// Creates a handle that completes when all dependencies have completed.
/// Returns an immediately-complete handle if `deps` is empty.
#[allow(dead_code)]
pub(crate) fn join(deps: &[Dependency]) -> TaskHandle<()> {
    let inner = Arc::new(TaskInner::new());
    let (tx, rx) = crossbeam_channel::bounded(1);

    if deps.is_empty() {
        let _ = tx.send(());
        inner.complete();
        return TaskHandle::new(rx, inner);
    }

    let remaining = Arc::new(AtomicU32::new(deps.len() as u32));

    for dep in deps {
        let remaining = Arc::clone(&remaining);
        let tx = tx.clone();
        let inner = Arc::clone(&inner);
        dep.inner.on_complete(move || {
            if remaining.fetch_sub(1, Ordering::Release) == 1 {
                std::sync::atomic::fence(Ordering::Acquire);
                let _ = tx.send(());
                inner.complete();
            }
        });
    }

    TaskHandle::new(rx, inner)
}

// ---------------------------------------------------------------------------
// Scheduler trait
// ---------------------------------------------------------------------------

/// Scheduler backend trait. Implementations provide the thread pool and
/// work distribution; callers use the stable API surface.
#[allow(dead_code)]
pub(crate) trait Scheduler: Send + Sync {
    /// Spawn a task on the pool. Returns a handle to wait on or poll.
    fn spawn<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> TaskHandle<T>;

    /// Spawn a task that runs only after all `deps` have completed.
    /// Uses atomic prerequisite counting — no global lock.
    fn spawn_after<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
        deps: &[Dependency],
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

// ---------------------------------------------------------------------------
// RayonScheduler
// ---------------------------------------------------------------------------

/// Rayon-backed scheduler. Work-stealing thread pool with parallel
/// iterators. Production-grade, zero custom scheduling code.
pub(crate) struct RayonScheduler {
    pool: Arc<rayon::ThreadPool>,
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

        Self {
            pool: Arc::new(pool),
        }
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

    /// Spawn a closure on the pool with completion tracking.
    fn spawn_inner<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
        tx: Sender<T>,
        inner: Arc<TaskInner>,
    ) {
        self.pool.spawn(move || {
            let result = f();
            let _ = tx.send(result);
            inner.complete();
        });
    }
}

impl Scheduler for RayonScheduler {
    fn spawn<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> TaskHandle<T> {
        let inner = Arc::new(TaskInner::new());
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.spawn_inner(f, tx, Arc::clone(&inner));
        TaskHandle::new(rx, inner)
    }

    fn spawn_after<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
        deps: &[Dependency],
    ) -> TaskHandle<T> {
        if deps.is_empty() {
            return self.spawn(f);
        }

        let inner = Arc::new(TaskInner::new());
        let (tx, rx) = crossbeam_channel::bounded(1);

        let remaining = Arc::new(AtomicU32::new(deps.len() as u32));
        type BoxedTask<T> = Arc<Mutex<Option<Box<dyn FnOnce() -> T + Send>>>>;
        let task: BoxedTask<T> = Arc::new(Mutex::new(Some(Box::new(f))));

        for dep in deps {
            let remaining = Arc::clone(&remaining);
            let task = Arc::clone(&task);
            let tx = tx.clone();
            let inner = Arc::clone(&inner);
            let pool = Arc::clone(&self.pool);

            dep.inner.on_complete(move || {
                if remaining.fetch_sub(1, Ordering::Release) == 1 {
                    std::sync::atomic::fence(Ordering::Acquire);
                    let f = task.lock().expect("poisoned").take().expect("task stolen");
                    pool.spawn(move || {
                        let result = f();
                        let _ = tx.send(result);
                        inner.complete();
                    });
                }
            });
        }

        TaskHandle::new(rx, inner)
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // -- Basic spawn/wait (from sw-e475fe) --------------------------------

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

    // -- Dependencies (sw-ac1601) -----------------------------------------

    #[test]
    fn spawn_after_single_dep() {
        let sched = RayonScheduler::new(2);
        let a = sched.spawn(|| 10u32);
        let dep = a.dependency();
        let b = sched.spawn_after(|| 20u32, &[dep]);
        assert_eq!(a.wait(), 10);
        assert_eq!(b.wait(), 20);
    }

    #[test]
    fn spawn_after_empty_deps() {
        let sched = RayonScheduler::new(2);
        let handle = sched.spawn_after(|| 42u32, &[]);
        assert_eq!(handle.wait(), 42);
    }

    #[test]
    fn spawn_after_chain() {
        // A → B → C: each adds to a shared counter
        let sched = RayonScheduler::new(2);
        let counter = Arc::new(AtomicUsize::new(0));

        let c1 = Arc::clone(&counter);
        let a = sched.spawn(move || {
            c1.fetch_add(1, Ordering::SeqCst);
        });

        let c2 = Arc::clone(&counter);
        let b = sched.spawn_after(
            move || {
                c2.fetch_add(10, Ordering::SeqCst);
            },
            &[a.dependency()],
        );

        let c3 = Arc::clone(&counter);
        let c = sched.spawn_after(
            move || {
                c3.fetch_add(100, Ordering::SeqCst);
            },
            &[b.dependency()],
        );

        c.wait();
        assert_eq!(counter.load(Ordering::SeqCst), 111);
    }

    #[test]
    fn spawn_after_diamond() {
        // A → B, A → C, B+C → D
        let sched = RayonScheduler::new(4);
        let log = Arc::new(Mutex::new(Vec::<&str>::new()));

        let l1 = Arc::clone(&log);
        let a = sched.spawn(move || {
            l1.lock().unwrap().push("A");
        });

        let l2 = Arc::clone(&log);
        let b = sched.spawn_after(
            move || {
                l2.lock().unwrap().push("B");
            },
            &[a.dependency()],
        );

        let l3 = Arc::clone(&log);
        let c = sched.spawn_after(
            move || {
                l3.lock().unwrap().push("C");
            },
            &[a.dependency()],
        );

        let l4 = Arc::clone(&log);
        let d = sched.spawn_after(
            move || {
                l4.lock().unwrap().push("D");
            },
            &[b.dependency(), c.dependency()],
        );

        d.wait();

        let order = log.lock().unwrap();
        // A must be first, D must be last. B and C can be in either order.
        assert_eq!(order[0], "A");
        assert_eq!(order[3], "D");
        assert!(order[1] == "B" || order[1] == "C");
        assert!(order[2] == "B" || order[2] == "C");
        assert_ne!(order[1], order[2]);
    }

    #[test]
    fn spawn_after_already_complete_dep() {
        let sched = RayonScheduler::new(2);
        let a = sched.spawn(|| 1u32);
        let dep = a.dependency();
        a.wait(); // a is complete before spawn_after

        let completed = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&completed);
        let b = sched.spawn_after(
            move || {
                c.store(true, Ordering::SeqCst);
            },
            &[dep],
        );
        b.wait();
        assert!(completed.load(Ordering::SeqCst));
    }

    #[test]
    fn spawn_after_preserves_result_type() {
        let sched = RayonScheduler::new(2);
        let a = sched.spawn(|| "hello".to_string());
        let dep = a.dependency();
        let b = sched.spawn_after(|| vec![1, 2, 3], &[dep]);
        assert_eq!(a.wait(), "hello");
        assert_eq!(b.wait(), vec![1, 2, 3]);
    }

    #[test]
    fn join_empty() {
        let handle = join(&[]);
        assert!(handle.is_complete());
        handle.wait();
    }

    #[test]
    fn join_multiple() {
        let sched = RayonScheduler::new(2);
        let a = sched.spawn(|| 1u32);
        let b = sched.spawn(|| 2u32);
        let c = sched.spawn(|| 3u32);

        let all = join(&[a.dependency(), b.dependency(), c.dependency()]);
        all.wait();

        // All tasks have completed (handles were not consumed by join)
        assert!(a.is_complete());
        assert!(b.is_complete());
        assert!(c.is_complete());
    }

    #[test]
    fn join_then_spawn_after() {
        let sched = RayonScheduler::new(2);
        let a = sched.spawn(|| 10u32);
        let b = sched.spawn(|| 20u32);

        let all = join(&[a.dependency(), b.dependency()]);
        let c = sched.spawn_after(|| 30u32, &[all.dependency()]);

        assert_eq!(c.wait(), 30);
    }

    #[test]
    fn dependency_from_dropped_handle() {
        let sched = RayonScheduler::new(2);
        let a = sched.spawn(|| 42u32);
        let dep = a.dependency();
        drop(a); // drop handle, task still runs

        let completed = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&completed);
        let b = sched.spawn_after(
            move || {
                c.store(true, Ordering::SeqCst);
            },
            &[dep],
        );
        b.wait();
        assert!(completed.load(Ordering::SeqCst));
    }
}
