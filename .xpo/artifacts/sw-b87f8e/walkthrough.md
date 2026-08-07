## What was built

A persistent `JobPool` in `crates/engine/src/jobs.rs` — engine-internal worker threads for OOC pipeline tasks. Initialized at boot, auto-sized to hardware, not exposed to games.

## Why

The BrickPager spawns its own threads. The OOC pipeline needs a shared pool that worldgen, meshing, streaming, and future engine tasks all use. One pool, sized at boot, with a single completion channel.

## How it works

**Workers** loop on a shared crossbeam channel, execute `FnOnce` closures, send `Box<dyn Any + Send>` results back through a completion channel.

**Auto-sizing**: `max(2, available_parallelism() - 2)` — leaves headroom for the main thread and OS. On a 10-core M1 Max: 8 workers. Overridable via `EngineConfig::worker_threads`.

**Submit/drain** lifecycle:
```rust
pool.submit(|| expensive_computation());
// ... later ...
let results = pool.drain_completed::<ComputeResult>();
```

`drain_completed` is non-blocking — returns whatever's ready, empty vec if nothing.

**Type erasure**: results are `Box<dyn Any + Send>`, downcast at drain time. Type mismatches are logged and dropped.

**Drop**: takes the sender (closing the channel), workers see `recv() -> Err` and exit, join all handles. Tests verify clean shutdown.

## Key decisions

- **`pub(crate)` only** — games never see the pool. A game-facing job API with priorities, cancellation, and budget control is a separate concern (sw-2c3e44).
- **No priority queues** — single FIFO. Priority comes when we have workloads that need it.
- **No work-stealing** — crossbeam's channel already distributes across workers.
- **No BrickPager migration** — the pager has a specialized request/response protocol. Migration is a separate story.
- **`Option<Sender>` for Drop** — `.take()` drops the sender before joining workers, avoiding the clone-and-drop-clone bug that caused the initial test hang.
