## What

A persistent `JobPool` initialized at engine boot. Workers pull jobs from a shared queue, execute them, and send results through a completion channel. The main thread drains results during update. No per-task thread spawning.

## Why

The BrickPager currently spawns its own threads with crossbeam channels — functional but not reusable. The OOC pipeline needs a general-purpose pool that worldgen, meshing, streaming, and game jobs (AI, pathfinding) all share. One pool, sized at boot, with a single completion channel the game drains.

## Acceptance Criteria

- [ ] `JobPool` struct with persistent worker threads
- [ ] Initialized at engine boot, thread count from `EngineConfig`
- [ ] `engine.submit(job)` to enqueue work from update
- [ ] `engine.drain_completed::<T>()` to collect results on the main thread
- [ ] Jobs are `FnOnce + Send` closures that return a boxed result
- [ ] Engine logs pool init (thread count)
- [ ] Unit tests for submit/drain lifecycle
- [ ] `make test` and `make lint` pass

## Design

### Job submission

```rust
engine.submit(move || {
    // heavy work on background thread
    let mesh = generate_mesh(chunk_id);
    mesh  // return value sent to completion channel
});
```

The closure is `FnOnce() -> Box<dyn Any + Send> + Send + 'static`. Workers execute it and send the result to the completion channel.

### Result collection

```rust
for mesh in engine.drain_completed::<MeshData>() {
    world.attach_mesh(chunk_id, mesh);
}
```

`drain_completed::<T>()` returns an iterator of `T` by downcasting from `Box<dyn Any + Send>`. Results that don't match `T` are skipped (logged at warn level). This lets different job types coexist on one pool.

### JobPool (engine-internal)

```rust
struct JobPool {
    sender: crossbeam_channel::Sender<Job>,
    results: crossbeam_channel::Receiver<Box<dyn Any + Send>>,
    workers: Vec<thread::JoinHandle<()>>,
}

type Job = Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>;
```

Workers loop on `sender.recv()`, execute the job, send the return value to `results`. When the pool drops, the sender closes, workers exit.

### EngineConfig

```rust
pub struct EngineConfig {
    // ... existing fields ...
    /// Worker thread count (default: available_parallelism - 2, min 2).
    pub worker_threads: usize,
}
```

Default: `available_parallelism() - 2` (leave headroom for main + render threads), minimum 2.

### Engine integration

```rust
impl Engine {
    pub fn submit<T: Send + 'static>(&self, job: impl FnOnce() -> T + Send + 'static);
    pub fn drain_completed<T: 'static>(&mut self) -> impl Iterator<Item = T> + '_;
}
```

`submit` wraps the job's return value in `Box<dyn Any + Send>` before enqueuing. `drain_completed` tries `recv` non-blockingly until the channel is empty, downcasts each result.

## Flow

1. **Create `jobs.rs`** — `JobPool`, worker spawn, submit, drain.
2. **Add to Engine** — `JobPool` field, initialized in `Engine::new`.
3. **Expose `submit` / `drain_completed`** on Engine.
4. **Log** — boot: "pool: N worker threads".
5. **Test** — submit jobs, verify results arrive via drain.
6. **Do NOT migrate BrickPager yet** — it has its own specialized channel protocol (request → result with grid_pos). Migration is a separate story when we integrate OOC streaming through the pool.

## Decisions

- **No priority queues yet** — single FIFO queue. Priority (frame-critical vs background) is future work. The FIFO is correct for the current workload (all jobs are equally important).

- **No work-stealing** — crossbeam's bounded channel already distributes work across workers. True work-stealing (per-worker deques with stealing from other workers) adds complexity for minimal gain when all jobs come from one producer.

- **`Box<dyn Any + Send>` for results** — type erasure lets different job types share one pool. The `drain_completed::<T>` downcast is the type-safe collection point. Alternative: separate channels per job type — more boilerplate, no real advantage.

- **No BrickPager migration** — the pager has a specialized request/response protocol (send grid_pos, receive LoadResult). Fitting that into the generic pool requires rethinking pager's internal state machine. Separate story.

- **Drop cleanup** — when `JobPool` drops, the sender channel closes. Workers see `recv() -> Err` and exit. `JoinHandle::join` in Drop ensures clean shutdown.

## Edge Cases

- **Zero results** — `drain_completed` returns an empty iterator when no jobs have completed. No blocking.
- **Type mismatch** — if `drain_completed::<Foo>` encounters a `Bar` result, it logs a warning and skips it. The `Bar` is lost — the caller should drain with the correct type.
- **Pool drop during in-flight jobs** — workers finish their current job, then exit when recv fails. Results from in-flight jobs are still delivered to the channel and drained on drop.
