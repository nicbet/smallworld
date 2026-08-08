## Task Dependencies with Atomic Prerequisite Counting

### What was built

Task dependency infrastructure for the job system, following the UE5 atomic prerequisite
counting pattern. Tasks can now express ordering constraints without blocking worker threads.

### New types

**`TaskInner`** (`jobs.rs:27`) — shared completion state for a task:
- `completed: AtomicBool` — set when the task finishes
- `on_complete: Mutex<Vec<Box<dyn FnOnce() + Send>>>` — callbacks fired on completion

When a task completes, it calls `TaskInner::complete()` which sets the flag and drains
all registered callbacks. The mutex is only touched twice per task lifetime (registration
and dispatch), never on the scheduling hot path.

**`Dependency`** (`jobs.rs:69`) — type-erased dependency handle wrapping `Arc<TaskInner>`.
Obtained via `TaskHandle::dependency()`. The dependency doesn't carry the result type —
it only signals "this task has finished." This allows heterogeneous dependency chains
(task returning `String` can depend on task returning `Vec<u32>`).

**`join(deps)`** (`jobs.rs:129`) — creates a `TaskHandle<()>` that completes when all
input dependencies complete. Uses the same atomic counting pattern: `AtomicU32` starts
at `deps.len()`, each dep's `on_complete` callback decrements, last decrement sends `()`
through the channel. Empty deps returns an immediately-complete handle.

### How dependencies resolve

`spawn_after(f, deps)` in `RayonScheduler`:

1. Creates `AtomicU32` initialized to `deps.len()`
2. Stores the task closure in `Arc<Mutex<Option<Box<...>>>>`
3. For each dependency, registers an `on_complete` callback that:
   - `fetch_sub(1, Release)` on the counter
   - If result was 1 (last decrement): `fence(Acquire)`, takes the closure, spawns on rayon
4. If a dependency is already complete when registering, the callback fires immediately

The fence pairing (`Release` on decrement, `Acquire` on zero-check) ensures all
predecessor writes are visible to the spawned task. This is the standard pattern from
UE5's `NumberOfPrerequistitesOutstanding`.

### Changes to existing code

- `TaskHandle<T>` gains `inner: Arc<TaskInner>` field and `dependency()` method
- `RayonScheduler.pool` changed from `rayon::ThreadPool` to `Arc<rayon::ThreadPool>` so
  `on_complete` callbacks can capture a pool reference for spawning
- `Scheduler` trait gains `spawn_after()` method
- `RayonScheduler::spawn` now calls `inner.complete()` after sending the result
- New helper `spawn_inner()` factors out the common spawn-with-completion pattern

### What was deferred

- **Task retraction** — rayon doesn't expose its internal deques, so we can't pull
  unstarted tasks back for inline execution. Deferred to sw-2ca320 (custom crossbeam
  scheduler). For now, `wait()` blocks on the channel.
- **Main-thread result channel** — dropped from scope. StreamStage already polls handles
  with `is_complete()`, making a separate channel redundant.

### Test coverage

10 new tests covering:
- Single dependency, empty deps, chain (A→B→C), diamond (A→B+C→D)
- Already-complete dependency, heterogeneous result types
- `join()` empty and multiple, `join` feeding `spawn_after`
- Dependency from a dropped handle (task still runs, callbacks still fire)

### Files changed

| File | Change |
|---|---|
| `crates/engine/src/jobs.rs` | `TaskInner`, `Dependency`, `join()`, `spawn_after()`, `Arc<rayon::ThreadPool>`, 10 new tests |
