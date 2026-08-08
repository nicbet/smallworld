## What

Introduce a two-layer job system: a `Scheduler` trait defining the task API, and a
`RayonScheduler` backend as the initial implementation. Migrate the existing `JobPool`
(fire-and-forget work queue with typed result drain) to the new abstraction.

## Why

The current `JobPool` is a bare crossbeam-channel work queue — no dependencies, no
parallel-for, no priorities, no cancellation. The research (docs/research/job-system-comparison.md)
shows that all three major engines separate the scheduler backend from the task API. This
lets us start with rayon (production-grade work-stealing, zero custom scheduling code) and
swap to a custom crossbeam-deque scheduler later (sw-2ca320) without changing any callers.

## How

### New types in `crates/engine/src/jobs.rs`

```rust
/// Opaque handle to a spawned task. Can be waited on or polled.
pub(crate) struct TaskHandle<T> { ... }

impl<T: Send + 'static> TaskHandle<T> {
    /// Blocks until the task completes and returns the result.
    pub(crate) fn wait(self) -> T;

    /// Returns true if the task has completed.
    pub(crate) fn is_complete(&self) -> bool;
}

/// The scheduler trait — swappable backend.
pub(crate) trait Scheduler: Send + Sync {
    /// Spawn a task on the pool. Returns a handle to wait on or poll.
    fn spawn<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> TaskHandle<T>;

    /// Parallel-for: split `range` into batches of `batch_size`, execute `f(index)`
    /// across pool workers. Blocks until all batches complete.
    fn parallel_for(
        &self,
        range: std::ops::Range<usize>,
        batch_size: usize,
        f: impl Fn(usize) + Send + Sync,
    );

    /// Number of worker threads.
    fn worker_count(&self) -> usize;
}

/// Rayon-backed scheduler.
pub(crate) struct RayonScheduler { pool: rayon::ThreadPool }
```

### TaskHandle implementation

`TaskHandle<T>` wraps a `crossbeam_channel::Receiver<T>` (oneshot pattern: capacity-1
bounded channel). `spawn()` creates a sender/receiver pair, moves the sender into the
closure, and sends the result on completion.

- `wait()` calls `recv()` (blocking).
- `is_complete()` calls `try_recv()` — if `Ok`, caches the value internally for the
  subsequent `wait()`. If `Err(Empty)`, returns false.

Using crossbeam rather than rayon's own join/scope because `TaskHandle` must be `Send` and
outlive the spawn site — rayon scopes require the closure to borrow from the enclosing
scope, which doesn't work for fire-and-forget patterns like StreamStage.

### RayonScheduler implementation

- `RayonScheduler::new(worker_count)` creates a `rayon::ThreadPool` with `worker_count`
  threads via `rayon::ThreadPoolBuilder`.
- `RayonScheduler::auto()` uses `available_parallelism - 4` (reserving main + GPU +
  audio + headroom), min 2.
- `spawn()` calls `self.pool.spawn()` with the closure.
- `parallel_for()` calls `self.pool.install(|| { rayon::iter::... })` using rayon's
  parallel iterator with chunk size = batch_size.
- `worker_count()` returns `self.pool.current_num_threads()`.

### Migration path

The existing `JobPool` API has two consumers:

1. **`Engine` struct** — owns `jobs: JobPool`, passes `&jobs` to StreamStage.
2. **`StreamStage::stream()`** — calls `jobs.submit(closure)` and `jobs.drain_completed::<T>()`.

Migration:

- `Engine.jobs` changes from `JobPool` to `RayonScheduler` (concrete type, not trait object
  — no need for dynamic dispatch yet).
- `StreamStage::stream()` parameter changes from `&JobPool` to `&RayonScheduler`.
- `submit(closure)` → `spawn(closure)` — returns `TaskHandle<T>` instead of fire-and-forget.

**Pending tracking (Option C):** Keep `HashSet<VolumeKey>` for the O(1) contains check
(same as today). Add `Vec<TaskHandle<(VolumeKey, MeshData, f32)>>` for holding handles.
Each frame, poll handles with `is_complete()`, collect completed results, remove their keys
from the HashSet. This preserves the existing contains-check performance while replacing
the type-erased `Box<dyn Any>` + downcast with typed handles.

- `drain_completed::<T>()` is removed entirely.
- `pending: HashSet<VolumeKey>` stays.
- New field: `pending_handles: Vec<TaskHandle<(VolumeKey, MeshData, f32)>>`
- On submit: `pending.insert(key)` + push handle to `pending_handles`.
- On drain: partition `pending_handles` by `is_complete()`, collect results from completed
  handles via `wait()`, remove keys from `pending`.

Remove the old `JobPool` struct entirely.

### EngineConfig

`EngineConfig.worker_threads` stays the same (`0` = auto-detect). The `Engine` constructor
passes it to `RayonScheduler::new()` or `RayonScheduler::auto()`.

### Dependencies

Add to workspace `Cargo.toml`:
```toml
rayon = "1"
```

Add to `crates/engine/Cargo.toml`:
```toml
rayon.workspace = true
```

`crossbeam-channel` is already a dependency (used for `TaskHandle`).

## Acceptance Criteria

- [ ] `Scheduler` trait defined with `spawn`, `parallel_for`, `worker_count`
- [ ] `RayonScheduler` implements `Scheduler`
- [ ] `TaskHandle<T>` with `wait()` and `is_complete()`
- [ ] `parallel_for` distributes work with configurable batch size
- [ ] StreamStage migrated: typed handles replace `Box<dyn Any>` + downcast
- [ ] Old `JobPool` struct removed
- [ ] `cargo test` passes — unit tests for spawn, parallel_for, wait, is_complete
- [ ] `cargo clippy` clean
- [ ] Engine boots and renders (stream stage functional with new scheduler)

## Deferred

- **Task dependencies** (sw-ac1601) — atomic prerequisite counting, `spawn_after`, task retraction
- **Priority levels** (sw-232d69) — urgent/normal/background tiers
- **Serial pipes** (sw-39f679) — FIFO pipe execution
- **Cancellation** (sw-120de1) — cooperative cancel tokens
- **Custom scheduler** (sw-2ca320) — crossbeam-deque work-stealing replacement

## Edge Cases

- **Zero-length range in parallel_for** — should be a no-op, not an error.
- **TaskHandle dropped without wait** — the task still runs to completion (fire-and-forget
  is valid). The result is simply discarded.
- **Shutdown ordering** — `RayonScheduler::drop` must wait for in-flight tasks. Rayon's
  `ThreadPool::drop` already does this.
- **StreamStage pending handles accumulation** — if extraction is slow, handles accumulate.
  This is the same behavior as the current `pending` HashSet — no change in semantics.
