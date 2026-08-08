## Scheduler Trait + Rayon Backend

### What was built

A two-layer job system replacing the original `JobPool`:

1. **`Scheduler` trait** (`jobs.rs:53`) — defines `spawn()`, `parallel_for()`, and `worker_count()`. This is the stable API surface that callers program against. When we later build a custom work-stealing scheduler on crossbeam-deque (sw-2ca320), only the backend changes — all callers stay the same.

2. **`RayonScheduler`** (`jobs.rs:82`) — implements `Scheduler` using `rayon::ThreadPool`. Work-stealing, thread naming (`sw-worker-N`), configurable thread count. `auto()` sizes to `cores - 4` (reserving main, GPU submission, audio, headroom), minimum 2 workers.

3. **`TaskHandle<T>`** (`jobs.rs:14`) — typed handle returned by `spawn()`. Wraps a bounded(1) crossbeam channel (oneshot pattern). `is_complete()` does a non-blocking `try_recv` and caches the result in a `Mutex<Option<T>>` for the subsequent `wait()`. `wait()` consumes self and blocks until the result arrives.

### How the pieces fit together

```
Engine
  └── jobs: RayonScheduler
          └── pool: rayon::ThreadPool (N workers)

StreamStage::stream(&impl Scheduler)
  ├── poll pending_handles: Vec<(VolumeKey, TaskHandle<ExtractionResult>)>
  │     └── is_complete() → wait() → ReadyEntry → GPU upload
  ├── scheduler.spawn(|| extractor.extract(...)) → TaskHandle
  │     └── pushed to pending_handles + key to pending HashSet
  └── pending: HashSet<VolumeKey>  (O(1) "is this key in flight?")
```

The `Scheduler` trait uses `impl Scheduler` (static dispatch) at the call site in `StreamStage::stream()`, not `dyn Scheduler`. No trait object overhead.

### Key decisions

**Mutex for TaskHandle cache:** The workspace denies `unsafe_code`, so `UnsafeCell` was not an option. `Mutex<Option<T>>` is always uncontended (single-owner access pattern), so the lock acquisition is effectively a single atomic CAS — negligible cost.

**Crossbeam channel for TaskHandle, not rayon scope:** `TaskHandle` must be `Send` and outlive the spawn site (fire-and-forget is valid — StreamStage stores handles across frames). Rayon's `scope` requires closures that borrow from the enclosing scope, which doesn't fit this pattern. The bounded(1) crossbeam channel acts as a typed oneshot.

**Option C for pending tracking:** `HashSet<VolumeKey>` stays for O(1) contains check (unchanged from before). `Vec<TaskHandle>` replaces the shared completion channel. This is the minimal migration — the HashSet answers "is this key in flight?" and the vec holds the handles. No type erasure, no `Box<dyn Any>` downcast.

**Thread count `cores - 4`:** Reserves capacity for the main thread, future dedicated GPU submission thread, future audio thread, and one core of headroom. On an 8-core machine this gives 4 workers; on 16 cores, 12 workers.

### Files changed

| File | Change |
|---|---|
| `crates/engine/src/jobs.rs` | Complete rewrite: `JobPool` → `Scheduler` trait + `RayonScheduler` + `TaskHandle<T>`. 8 unit tests. |
| `crates/engine/src/stream.rs` | `&JobPool` → `&impl Scheduler`. `drain_completed()` → handle polling. Added `pending_handles` field and `ExtractionResult` type alias. |
| `crates/engine/src/engine.rs` | `JobPool` → `RayonScheduler` in struct and both constructors. |
| `Cargo.toml` | Added `rayon = "1"` to workspace dependencies. |
| `crates/engine/Cargo.toml` | Added `rayon.workspace = true`. |
