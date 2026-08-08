# Job System Comparison: UE5 vs Unity vs Godot 4

Comparative analysis for designing smallworld's job system. Each engine made different
tradeoffs — this distills what works, what doesn't, and what applies to a Rust voxel engine.

See individual reports for full details:
- [UE5 Task System](ue5-task-system.md)
- [Unity Job System](unity-job-system.md)
- [Godot 4 Threading](godot4-threading-architecture.md)

---

## Architecture Comparison

| | UE5 | Unity | Godot 4 |
|---|---|---|---|
| **Model** | Two-layer: DAG scheduler + work-stealing pool | Struct-based jobs + NativeContainers + Burst | Centralized FIFO pool + server actors |
| **Scheduling** | Chase-Lev work-stealing deques | Work-stealing on batch ranges | Centralized FIFO, single BinaryMutex |
| **Thread count** | cores - 1 (+ named threads) | cores - 1 | cores - 1 |
| **Priority** | 5 levels (High → BackgroundLow) | 2 (normal + batched scheduling) | 2 tiers (high/low) with promotion |
| **Dependencies** | Atomic prerequisite counter per task | JobHandle chains + CombineDependencies | Semaphore-based wait + collaborative execution |
| **Safety** | Named-thread isolation (game/render/RHI) | AtomicSafetyHandle + compile-time checks | Thread guards (debug-only) + server isolation |
| **Parallelism ceiling** | High (mature pipeline, frame overlap) | Very high (SIMD + zero-GC + chunk iteration) | Moderate (inter-server only) |
| **Complexity** | High | Very high | Low |

---

## Best Practices Distilled

### 1. Two-Layer Architecture (UE5)

Every engine benefits from separating the **scheduler** (thread pool, work distribution)
from the **task API** (dependency DAGs, parallel-for, pipes). UE5 proves this: they shipped
a new high-level API (`UE::Tasks`) without changing the scheduler (`FScheduler`), and both
old and new APIs coexist on the same pool.

**For smallworld:** Build a low-level `Scheduler` (thread pool + work-stealing) and layer
higher-level constructs on top (DAG tasks, parallel-for, serial pipes). This lets us evolve
the API without rebuilding the pool.

### 2. Work Stealing (UE5, Unity)

Both UE5 and Unity use work-stealing. Godot uses FIFO and pays for it on high-core-count
machines (O(N) notification, single-lock contention). Work-stealing is the consensus
best-in-class approach for game engines:

- **LIFO locally** (cache-warm, depth-first)
- **FIFO stealing** (coarse-grained, breadth-first)
- No global lock on the critical path

Godot's FIFO is simpler but has a known scaling ceiling (~24 cores). For a voxel engine
with embarrassingly-parallel workloads (brick streaming, SVO construction, meshing), work
stealing is essential.

**For smallworld:** Chase-Lev work-stealing deques, one per worker thread. Use `crossbeam-deque`
or a custom implementation.

### 3. Atomic Prerequisite Counting (UE5)

UE5's dependency resolution is fully decentralized: each task has an `AtomicU32`
outstanding-prerequisite counter. Predecessors decrement it; zero triggers queueing. No
global scheduler lock on the critical path. This is simpler and more scalable than
centralized dependency tracking.

**For smallworld:** `AtomicU32` with `fetch_sub(1, Release)` + `fence(Acquire)` on zero.
Direct mapping to Rust.

### 4. Copy-on-Schedule / Value Semantics (Unity)

Unity's copy-on-schedule is the single best idea in their job system: the entire job struct
is memcpy'd to unmanaged memory at schedule time, creating complete data isolation. No
synchronization needed on job-local data. Only shared buffers (NativeContainers) need
safety tracking.

**For smallworld:** Rust's `Send + 'static` bound on job closures/structs achieves the same
thing. The borrow checker enforces this at compile time — no runtime safety handles needed.
This is Rust's biggest advantage over all three engines.

### 5. Named/Pinned Threads (UE5)

Game thread, render thread, RHI thread, audio thread — each is a dedicated thread with
exclusive access to its subsystem. This eliminates per-call locking for subsystem access.
UE5's frame pipeline (Game N+1 | Render N | GPU N-1) trades one frame of latency for
race-free inter-system communication.

Godot's "server" pattern achieves the same thing through message-passing: servers own their
data and process commands sequentially.

**For smallworld:** Dedicated threads for GPU submission and audio. Everything else goes
through the work-stealing pool. The OOC pipeline stages already have this shape — each
stage produces a buffer consumed by the next.

### 6. Per-Thread Wake-Up (Unity 2022.2+)

All three engines have struggled with thread wake-up costs. Unity's evolution is
instructive:

- Global semaphore → O(N) cost per wake
- "Whipping boy" spinning thread → wastes one core
- Per-thread futex (`WaitOnAddress`) → O(1) per wake, scales to 32+ cores

**For smallworld:** `parking_lot::Condvar` per worker, or raw futex on Linux / `os_unfair_lock`
on macOS. Avoid global semaphores.

### 7. Task Retraction / Collaborative Waiting (UE5, Godot)

When a worker thread waits on an incomplete task, it should not just block. UE5 uses **task
retraction** (steal and execute the waited-on task inline). Godot uses **collaborative
waiting** (pick up other work from the queue while waiting). Both prevent deadlock in a
fixed-size pool.

**For smallworld:** Task retraction is simpler and more effective. If you `wait()` on a task
that hasn't started, retract it and run it on the current thread. This prevents the
all-workers-blocked scenario.

### 8. Serial Pipes (UE5 FPipe)

UE5's `FPipe` gives sequential execution semantics without a dedicated thread or mutex.
Tasks on the same pipe run FIFO but can migrate between workers. This is perfect for
serialized access to a thread-unsafe resource — no priority inversion, no deadlock risk.

**For smallworld:** An MPSC channel feeding a task that runs on any available worker.
Useful for serialized GPU command recording, asset loading queues, etc.

### 9. ParallelFor with Batch Size Control (UE5, Unity)

Both UE5 and Unity expose parallel-for with configurable batch sizes. The batch size is
critical — too small and the atomic overhead dominates, too large and you get poor load
balancing. Unity's rule of thumb: each batch should represent **at least a few microseconds
of work**.

**For smallworld:** Expose `parallel_for(range, batch_size, |i| { ... })` with sensible
defaults. For voxel work: brick streaming batches of 16-64, meshing batches of 1-4
(expensive per-item), LOD evaluation batches of 32-128.

### 10. Priority with Starvation Prevention (Godot)

Godot's two-tier priority with promotion is simple and effective. Low-priority tasks park
in a separate queue. When high-priority work completes and a slot opens, a low-priority
task is promoted. This prevents indefinite starvation without complex priority queues.

**For smallworld:** Three priority levels are sufficient:
- **Urgent** — physics, audio feed, frame-critical work
- **Normal** — streaming, meshing, LOD, culling
- **Background** — compaction, saves, worldgen prefetch

Low-priority promotion when slots open. UE5's 5-level scheme is overkill.

---

## Anti-Patterns to Avoid

### 1. Single Global Mutex (Godot)

Godot's `BinaryMutex` on the task queue creates contention on high-core-count machines.
O(N) notification per task completion. Known to cause ~30x slowdowns for background asset
loading when the render server holds the mutex during frame draws.

### 2. Debug-Only Safety (Godot)

Thread guards compiled out in release builds means race conditions can ship to players.
Unity's approach (safety in dev builds, compiled out in release) is the same pattern but
they at least catch issues during development. Rust's borrow checker eliminates this
category entirely — safety is a compile-time property, not a runtime check.

### 3. Mandatory Join for Memory Reclamation (Godot)

Every task must be `wait_for_task_completion()`d to reclaim memory. No fire-and-forget.
This is a design limitation of their PagedAllocator. Use arena allocation with epoch-based
reclamation instead (`crossbeam-epoch` or similar).

### 4. Runtime Safety Overhead (Unity)

Unity's AtomicSafetyHandle checks add measurable overhead in development builds. They're
necessary because C# can't express ownership at compile time. Rust doesn't need this —
the type system already prevents conflicting access. Don't replicate Unity's runtime safety
layer in Rust; the borrow checker is strictly better.

### 5. Frame Pipeline Without Task Retraction (deadlock risk)

If workers can block on other tasks and you have a fixed-size pool, you need either task
retraction (UE5) or collaborative waiting (Godot) to prevent deadlock. A naive "schedule
and wait" pattern on a fixed pool will deadlock under load.

---

## What Smallworld Needs

Based on the comparison, the job system for smallworld should combine the best ideas:

### Core Design

1. **Work-stealing thread pool** — Chase-Lev deques, cores-1 workers, per-thread futex
   wake-up. Not FIFO, not global mutex.

2. **Three priority levels** — urgent/normal/background with promotion to prevent starvation.

3. **Atomic prerequisite counting** for task dependencies — decentralized, no global lock.

4. **Task retraction** on wait — if the waited task hasn't started, retract and execute
   inline. Prevents deadlock.

5. **ParallelFor** with configurable batch size — the bread-and-butter primitive for
   data-parallel voxel work.

6. **Serial pipes** — MPSC channel + sequential execution for serialized access patterns
   (GPU commands, asset loading).

### Rust-Specific Advantages

- **No runtime safety system needed** — `Send + 'static` on tasks, borrow checker for
  shared data access. Unity's AtomicSafetyHandle and Godot's thread guards are unnecessary.
- **Zero-cost closures** — unlike Unity (no lambdas in jobs) or UE5 (virtual dispatch),
  Rust closures are monomorphized and inlined.
- **`crossbeam-deque`** for work-stealing, **`parking_lot`** for synchronization —
  battle-tested crates that match or exceed what these engines built from scratch.
- **`std::simd`** (nightly) or **`glam`** for SIMD math — Rust already compiles through
  LLVM, so Burst's advantage (LLVM + no GC) is Rust's default.

### Thread Layout

```
Main thread          — game logic, input, frame orchestration
GPU submission       — dedicated, receives command buffers via channel
Audio thread         — dedicated, lock-free ring buffer feed
Worker pool (N-3)    — work-stealing, handles all parallel work:
                       streaming, meshing, culling, LOD, physics,
                       scripting, worldgen, saves
```

### Build vs. Buy

| Component | Recommendation | Rationale |
|---|---|---|
| Thread pool + work-stealing | `rayon` or custom on `crossbeam-deque` | rayon is production-grade but opaque; custom gives control over priority, retraction |
| Synchronization | `parking_lot` | Futex-based, faster than `std::sync` on all platforms |
| Channels | `crossbeam-channel` or `flume` | For named-thread communication (main ↔ GPU, main ↔ audio) |
| Atomics | `std::sync::atomic` | Standard library is sufficient |
| SIMD math | `glam` | Already widely used in Rust gamedev, maps to hardware SIMD |

### Rayon vs. Custom

**Rayon** is the obvious starting point — it's a production-grade work-stealing pool with
`par_iter`, `join`, and `scope`. However:

- No priority levels (everything is equal priority)
- No task retraction (uses `scope` for deadlock avoidance instead)
- No serial pipes
- No per-task cancellation tokens
- Global pool model (one pool per process)

For a game engine, a **custom scheduler built on `crossbeam-deque`** gives control over
priority, retraction, named threads, and cancellation. Rayon is fine for prototyping but
we'll likely outgrow it.

**Recommendation:** Start with rayon for the initial Job System stories (get parallel-for
and basic task spawning working). Plan to replace with a custom scheduler when we need
priority levels, task retraction, or serial pipes. The task API layer stays the same either
way — that's the value of the two-layer architecture.

---

## Sources

- [UE5 Task System](ue5-task-system.md) — full architecture report
- [Unity Job System](unity-job-system.md) — full architecture report
- [Godot 4 Threading](godot4-threading-architecture.md) — full architecture report
