//! Persistent worker pool for engine-internal background work.
//!
//! The pool is initialized at engine boot and serves OOC pipeline tasks
//! (streaming, worldgen, meshing). It is **not** exposed to games — a
//! game-facing job API with priorities, cancellation, and budget control
//! is a separate concern.

use std::any::Any;
use std::thread;

use crossbeam_channel::{Receiver, Sender};

type Job = Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>;

/// Persistent worker pool. Workers pull jobs from a shared queue and
/// send results through a completion channel.
#[allow(dead_code)]
pub(crate) struct JobPool {
    job_tx: Option<Sender<Job>>,
    result_rx: Receiver<Box<dyn Any + Send>>,
    workers: Vec<thread::JoinHandle<()>>,
}

#[allow(dead_code)]
impl JobPool {
    /// Creates a pool with `count` worker threads.
    pub(crate) fn new(count: usize) -> Self {
        let count = count.max(1);
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<Job>();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();

        let workers = (0..count)
            .map(|i| {
                let rx = job_rx.clone();
                let tx = result_tx.clone();
                thread::Builder::new()
                    .name(format!("sw-worker-{i}"))
                    .spawn(move || {
                        while let Ok(job) = rx.recv() {
                            let result = job();
                            if tx.send(result).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("failed to spawn worker thread")
            })
            .collect();

        log::info!("boot: job pool with {count} worker threads");

        Self {
            job_tx: Some(job_tx),
            result_rx,
            workers,
        }
    }

    /// Creates a pool sized for the current hardware.
    pub(crate) fn auto() -> Self {
        let cores = thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4);
        let workers = (cores.saturating_sub(2)).max(2);
        Self::new(workers)
    }

    /// Submits a job to the pool. The closure runs on a worker thread
    /// and its return value is sent to the completion channel.
    pub(crate) fn submit<T: Send + 'static>(&self, job: impl FnOnce() -> T + Send + 'static) {
        let boxed: Job = Box::new(move || Box::new(job()));
        self.job_tx
            .as_ref()
            .expect("job pool shut down")
            .send(boxed)
            .expect("job pool shut down");
    }

    /// Drains all completed results of type `T`, non-blocking.
    /// Results that don't match `T` are logged and dropped.
    pub(crate) fn drain_completed<T: 'static>(&self) -> Vec<T> {
        let mut results = Vec::new();
        while let Ok(boxed) = self.result_rx.try_recv() {
            match boxed.downcast::<T>() {
                Ok(val) => results.push(*val),
                Err(_) => log::warn!("job result type mismatch, dropped"),
            }
        }
        results
    }

    /// Number of worker threads.
    pub(crate) fn worker_count(&self) -> usize {
        self.workers.len()
    }
}

impl Drop for JobPool {
    fn drop(&mut self) {
        self.job_tx.take();
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_and_drain() {
        let pool = JobPool::new(2);
        pool.submit(|| 42u32);
        pool.submit(|| 99u32);

        std::thread::sleep(std::time::Duration::from_millis(50));

        let results = pool.drain_completed::<u32>();
        assert_eq!(results.len(), 2);
        assert!(results.contains(&42));
        assert!(results.contains(&99));
    }

    #[test]
    fn drain_empty_returns_nothing() {
        let pool = JobPool::new(1);
        let results = pool.drain_completed::<u32>();
        assert!(results.is_empty());
    }

    #[test]
    fn auto_creates_at_least_two() {
        let pool = JobPool::auto();
        assert!(pool.worker_count() >= 2);
    }

    #[test]
    fn type_mismatch_skipped() {
        let pool = JobPool::new(1);
        pool.submit(|| "hello".to_string());

        std::thread::sleep(std::time::Duration::from_millis(50));

        let results = pool.drain_completed::<u32>();
        assert!(results.is_empty());
    }

    #[test]
    fn drop_waits_for_workers() {
        let pool = JobPool::new(2);
        pool.submit(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            true
        });
        drop(pool);
    }
}
