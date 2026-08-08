## What

Add task dependencies to the job system: `spawn_after()` schedules a task that only
executes once all prerequisites complete, using atomic prerequisite counting (UE5 pattern).
Add `join()` to combine multiple handles.

## Why

The streaming pipeline currently fires independent tasks. As the engine grows (meshing →
LOD → upload chains, worldgen → brick-fill → SVO-insert pipelines), tasks need to express
ordering without blocking worker threads. Atomic prerequisite counting is decentralized —
no global lock on the critical path.

## How

### Completion callback infrastructure

Extend the task system with a shared, type-erased completion signal that can trigger
downstream work:

```rust
struct TaskInner {
    completed: AtomicBool,
    on_complete: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}
```

When a task finishes:
1. Send typed result through the channel (existing behavior)
2. Set `completed = true`
3. Drain and run all `on_complete` callbacks

`TaskHandle<T>` gains an `inner: Arc<TaskInner>` field.

### Dependency type

A type-erased dependency handle (the dependency doesn't care about result type):

```rust
pub(crate) struct Dependency {
    inner: Arc<TaskInner>,
}

impl<T: Send + 'static> TaskHandle<T> {
    pub(crate) fn dependency(&self) -> Dependency;
}
```

### spawn_after

New method on the `Scheduler` trait:

```rust
fn spawn_after<T: Send + 'static>(
    &self,
    f: impl FnOnce() -> T + Send + 'static,
    deps: &[Dependency],
) -> TaskHandle<T>;
```

Implementation (atomic prerequisite counting):
1. If `deps` is empty, delegate to `spawn()`.
2. Create `Arc<PendingTask>` holding:
   - `remaining: AtomicU32` initialized to `deps.len()`
   - `task: Mutex<Option<Box<dyn FnOnce() -> T + Send>>>` holding the closure
   - `result_tx: Sender<T>` for the result channel
   - `task_inner: Arc<TaskInner>` for the new task's own completion callbacks
3. For each dependency, register an `on_complete` callback that:
   - Calls `remaining.fetch_sub(1, Release)`
   - If the result is 1 (was last), calls `fence(Acquire)`, takes the closure, and
     spawns it on the rayon pool
4. For dependencies already completed (`completed.load(Acquire) == true`), run the
   callback immediately.

`RayonScheduler` changes its pool field to `Arc<rayon::ThreadPool>` so callbacks can
capture a pool reference.

### join

Free function that creates a handle completing when all inputs complete:

```rust
pub(crate) fn join(deps: &[Dependency]) -> TaskHandle<()>;
```

Implementation: same atomic counting pattern. When the last dependency fires, send
`()` through the result channel.

### Task retraction

**Deferred to sw-2ca320 (custom scheduler).** Rayon doesn't expose its internal queues,
so we can't retract tasks. With rayon, `wait()` simply blocks on the channel. Rayon's
cooperative work-stealing prevents complete thread starvation in practice. True retraction
(pull unstarted task from deque, execute inline) requires the custom Chase-Lev scheduler.

### Main-thread result channel

**Dropped from scope.** StreamStage already uses `TaskHandle` polling with `is_complete()`.
A separate result channel would duplicate this mechanism. If a fire-and-forget-with-callback
pattern is needed later, it can be added as a separate primitive.

## Changes to existing code

- `TaskHandle<T>` gains `inner: Arc<TaskInner>` field
- `RayonScheduler.pool` changes from `rayon::ThreadPool` to `Arc<rayon::ThreadPool>`
- `Scheduler` trait gains `spawn_after()` method
- New `Dependency` struct
- New `join()` free function

No changes to `StreamStage` or `Engine` — this is purely additive.

## Acceptance Criteria

- [ ] `spawn_after(f, &[dep])` only runs `f` after all deps complete
- [ ] Atomic prerequisite decrement — no global lock on the critical path
- [ ] `join()` combines dependencies into a single handle
- [ ] Already-completed dependencies are handled correctly (no hang)
- [ ] Empty deps list in `spawn_after` behaves like `spawn`
- [ ] Diamond dependency pattern works (A → B, A → C, B+C → D)
- [ ] `cargo test` — unit tests for chains, diamonds, join, already-complete deps
- [ ] `cargo clippy` clean

## Edge Cases

- **All deps already complete when spawn_after is called** — task spawns immediately.
- **Single dependency** — works (counter starts at 1, decrements to 0).
- **join with empty slice** — returns immediately-complete handle.
- **Dependency from a dropped handle** — the task still ran, `TaskInner` still exists
  via Arc, completion callbacks still fire.
