# Unity C# Job System & Burst Compiler Architecture

Technical deep-dive into Unity's job system internals for engine developers.
Focused on the native scheduling layer and safety system, not gameplay patterns.

---

## 1. Architecture

### Core Interfaces

The job system provides a small set of interfaces that job structs must implement:

- **IJob** -- Single-execution job. Runs on one worker thread. Has a single `Execute()`
  method with no parameters. Used for work that cannot be parallelized.
- **IJobFor** (replaced IJobParallelFor in newer versions) -- Parallel-for job.
  `Execute(int index)` is called once per index in a range. Indices are split into batches;
  worker threads grab batches from a shared work-stealing queue. Three scheduling modes:
  `Run()` (main thread, immediate), `Schedule()` (single worker, deferred),
  `ScheduleParallel()` (multiple workers, deferred).
- **IJobParallelFor** (legacy) -- Similar to IJobFor but with a fixed batch size specified
  at schedule time. The batch size determines the minimum number of indices processed per
  work-stealing unit.
- **IJobParallelForTransform** -- Specialized for Transform hierarchies; provides parallel
  access to TransformAccessArray with special native-side scheduling.

All job types are **interfaces implemented by user-defined structs**, not classes. This is
a fundamental architectural decision (detailed in section 7).

### Worker Thread Pool Design

The native job system creates **one worker thread per logical CPU core, minus one** (the
main thread occupies one core). The intent is 1:1 mapping between workers and cores to avoid
context switching overhead. On a 16-core machine, there are 15 worker threads.

The thread pool is created at engine startup and persists for the application lifetime.
Threads are never created or destroyed during normal operation.

### Internal Scheduling Pipeline

When a job is scheduled from C#, the following happens:

1. **Struct copy**: The C# job binding layer copies the entire job struct into an unmanaged
   memory allocation via `UnsafeUtility.AddressOf(ref jobData)`. This creates an isolated
   copy visible only to the executing job -- the caller's copy is independent. This
   copy-on-schedule semantic eliminates data races on job fields.
2. **JobScheduleParameters construction**: A `JobScheduleParameters` struct is assembled
   containing: a pointer to the copied job data, a pointer to JobReflectionData (opaque
   native metadata), the dependency JobHandle, and the scheduling mode.
3. **NativeContainer patching**: The native side uses the JobReflectionData to discover all
   NativeContainer fields in the struct via reflection metadata. It patches the
   AtomicSafetyHandle on each container to track that this job will read/write that
   container. For parallel jobs, it also applies buffer range restrictions
   (`PatchBufferMinMaxRanges`) to limit which indices each batch can touch.
4. **Safety validation** (editor/development builds only): The system checks for conflicting
   access -- if another already-scheduled, incomplete job writes to the same NativeContainer
   without a dependency chain between them, an exception is thrown at schedule time.
5. **Enqueue**: The job is placed in the native job queue. For batched mode, the job is not
   immediately dispatched -- it is held until `JobHandle.ScheduleBatchedJobs()` is called or
   the frame boundary triggers a flush.
6. **Worker wake-up**: When the batch is flushed, worker threads are signaled. The system
   determines how many workers to wake based on the job type.

### Work Stealing

Parallel jobs split their index range into batches. Each batch goes onto a queue. Worker
threads grab batches using **atomic operations** (no locks). When a worker finishes its
batch, it attempts to steal work from other workers' queues.
`JobsUtility.GetWorkStealingRange(ref ranges, jobIndex, out begin, out end)` returns the
next range to process, returning false when all work is complete.

Work stealing uses atomic compare-and-swap on range boundaries, not mutex locks. Critical
for low contention at high core counts.

### Worker Thread Wake-Up Evolution

This area saw major evolution:

- **Pre-2022 (old system)**: A **global semaphore** was used to wake all waiting workers.
  Releasing a semaphore on Windows has cost proportional to the number of waiting threads.
  On 32-core machines, waking all threads was substantial overhead.
- **2021.3 ("whipping boy" approach)**: One designated worker thread **spins continuously**
  (busy-waits), monitoring an atomic counter for new work. When work appears, this spinning
  thread wakes other workers as needed. Avoids the expensive N-thread semaphore release,
  trading one core's utilization for dramatically lower wake-up latency.
- **2022.2+ (current system)**: **Per-worker-thread synchronization primitives** using
  `WaitOnAddress`/`WakeByAddress` on Windows (futex-like API). Each worker has its own
  address to wait on, so waking one worker is O(1) regardless of how many others are
  sleeping. Scales well to high core counts.
- **2022.3.62f1 / 6000.0.48f1**: Further optimizations specifically for machines with 16+
  cores, reducing thread waking overhead.

### JobType Enum

Internally only **two** native job types exist:
- `JobType.Single` -- One `Execute()` call
- `JobType.ParallelFor` -- Multiple `Execute()` calls with index ranges

All higher-level job types (IJobChunk, IJobEntity, etc.) are built on top of these two
primitives in managed C#.

---

## 2. Safety System

### AtomicSafetyHandle

The core safety primitive. A struct with three fields:

- **`versionNode`** (`IntPtr`) -- Pointer to a native-side version tracking node. This node
  holds the authoritative access state.
- **`version`** (`int`) -- A local copy of the version at the time the handle was created.
  Used for fast comparison.
- **`staticSafetyId`** (`int`) -- Identifier for custom error messages.

Bit flag system for access tracking:

```
Read    = 1 << 0  // Read access permitted
Write   = 1 << 1  // Write access permitted
Dispose = 1 << 2  // Disposal permitted
```

Permission checks use bitmask comparisons:
- `CheckReadAndThrow()` verifies `version == (*versionPtr & ReadCheck)` where
  `ReadCheck = ~(Write | Dispose)`. If bits diverge, another job has write or dispose access.
- `CheckWriteAndThrow()` verifies `version == (*versionPtr & WriteCheck)`, blocking if any
  other job reads or writes.
- `CheckDeallocateAndThrow()` uses `ReadWriteDisposeCheck` to block disposal while any job
  accesses the data.

All check methods are `[MethodImpl(MethodImplOptions.AggressiveInlining)]` for zero overhead
in the hot path.

When scheduling a job, the scheduler modifies the version node bits to reflect the new job's
access pattern. If the new access conflicts with existing incomplete jobs, the version
comparison fails and an exception is thrown **at schedule time**.

At runtime (development builds only), every NativeContainer access calls
CheckReadAndThrow/CheckWriteAndThrow. These are compiled out in release builds via
`#if ENABLE_UNITY_COLLECTIONS_CHECKS`.

### Secondary Version System

Secondary versions handle container mutations that invalidate derived views:
- A NativeList can produce a NativeArray view of its contents.
- If the NativeList resizes (reallocating its buffer), the NativeArray view becomes dangling.
- The secondary version increments on resize, and any NativeArray derived from the old
  version detects the mismatch.

### DisposeSentinel

Managed-side leak detector. When a NativeContainer is allocated, a weak reference is
registered. If the container is garbage-collected without being disposed, the finalizer fires
and logs a leak warning. Modern versions use `UnsafeUtility.MallocTracked`/`FreeTracked`
instead.

### The "No Managed References in Jobs" Rule

Jobs cannot contain any managed types: classes, strings, delegates, arrays (managed),
interfaces, or anything that the GC can move. Reasons:

1. **Memory safety**: Managed objects can be relocated by the GC at any time.
2. **Blittability**: Job structs must be bitwise-copyable because they are memcpy'd to
   unmanaged memory at schedule time.
3. **Burst compatibility**: Burst operates entirely outside the GC.

---

## 3. Scheduling -- JobHandle Dependencies

### Dependency Chain

`JobHandle` is a value type representing a scheduled (or completed) job. When you call
`job.Schedule(dependency)`, the new job will not begin execution until `dependency` has
finished. Dependencies are transitive.

Multiple dependencies are combined via `JobHandle.CombineDependencies(handle1, handle2)`.

### Complete() Semantics

Calling `handle.Complete()` does the following in order:

1. **Recursively completes all dependencies**.
2. **Blocks the calling thread** until the job execution finishes.
3. **Releases the AtomicSafetyHandle locks** on all NativeContainers used by the job.
4. **Removes the job from the internal queue**.

After `Complete()` returns, the main thread can safely read/write all NativeContainers. This
is a **synchronization point** -- it stalls the main thread. Schedule early, complete late.

### ScheduleBatchedJobs()

Scheduling a job does not immediately dispatch it. Jobs are accumulated in a batch buffer.
`JobHandle.ScheduleBatchedJobs()` flushes the buffer and wakes workers. This batching avoids
redundant thread wake-ups when scheduling many jobs in a loop.

---

## 4. Integration with ECS (DOTS)

### Archetype-Chunk Memory Layout

Entities with the same set of component types share an **archetype**. Each archetype stores
its entities in **chunks of 16,384 bytes (16 KB)**. Within a chunk, component data is
stored as **parallel arrays** (Structure of Arrays / SoA):

```
Chunk (16 KB):
  [Entity IDs:   e0, e1, e2, ..., eN]
  [Position:     p0, p1, p2, ..., pN]
  [Velocity:     v0, v1, v2, ..., vN]
  [Health:       h0, h1, h2, ..., hN]
```

The 16 KB chunk size fits in L1 cache (typically 32-64 KB).

### IJobChunk

The ECS-native parallel job type. `Execute(in ArchetypeChunk chunk, int unfilteredChunkIndex,
bool useEnabledMask, in v128 chunkEnabledMask)` is called **once per matching chunk**. The
chunk-level granularity means:

- Optional components are checked once per chunk, not per entity.
- The job system distributes chunks across workers.
- Cache lines align with chunk layout.

### SystemState.Dependency

Each system has a `Dependency` property (a JobHandle). The ECS runtime manages this
automatically -- it combines dependency handles from all systems that access the same
component types, creating an automatic dependency graph across the entire system update
order without manual tracking.

### Structural Changes and EntityCommandBuffer

Jobs cannot modify the archetype graph (create/destroy entities, add/remove components)
because these operations invalidate chunk pointers. Instead, jobs record deferred commands
in an `EntityCommandBuffer`, which is played back on the main thread at a synchronization
point.

---

## 5. Burst Compiler

### Compilation Pipeline

1. **C# source** -> .NET IL via Roslyn
2. **Burst frontend** validates against HPC# restrictions
3. **IL-to-LLVM-IR translation**
4. **LLVM optimization passes** -- inlining, unrolling, constant folding, auto-vectorization
5. **Platform-specific code generation** -- SSE2/SSE4.2/AVX2 on x86, NEON on ARM64

### HPC# (High Performance C#) Subset

Burst only compiles a restricted subset of C#:

**Allowed:** Value types, enums, primitives, pointers, all control flow, ref/out, unsafe,
extension methods, IDisposable, try/finally, DllImport.

**Banned:** All managed reference types (classes, strings, delegates, managed arrays),
virtual/interface calls, boxing, catch blocks, storing to static fields, any GC allocation,
LINQ, reflection.

### SIMD Auto-Vectorization

`Unity.Mathematics` types (float4, float3, int4, etc.) map directly to hardware SIMD
registers. For manual control, Burst exposes hardware intrinsics via
`Unity.Burst.Intrinsics`.

### Performance Characteristics

Benchmarked speedups (Burst vs. standard C#/Mono):
- Vector3 addition (1M elements): **40x**
- Matrix multiplication (100K): **66x**
- Physics integration (10K): **23x**
- Terrain generation: **32x** (220ms to 6.8ms)

---

## 6. NativeContainer Types

| Type | Description | Parallel Write | Notes |
|------|-------------|---------------|-------|
| `NativeArray<T>` | Fixed-size contiguous array | Index-partitioned | Foundational type |
| `NativeList<T>` | Resizable array | `AsParallelWriter()` -- append-only | Must pre-allocate capacity |
| `NativeParallelHashMap<K,V>` | Unordered key-value map | `AsParallelWriter()` | Must pre-allocate capacity |
| `NativeQueue<T>` | FIFO queue | `AsParallelWriter()` | Dequeue is single-threaded only |
| `NativeStream` | Per-thread append buffers | Each thread writes to its own index | **Only deterministic parallel write** |
| `NativeReference<T>` | Single value | No parallel write | Useful for atomic counters |

**Allocator Types:**
- **`Allocator.Temp`** -- Frame-scoped, auto-deallocated. Cannot be passed to jobs.
- **`Allocator.TempJob`** -- Must be disposed within 4 frames. Thread-local ring buffer.
- **`Allocator.Persistent`** -- Manual lifetime. Standard heap allocation.

**ParallelWriter Internals:** Uses `[NativeSetThreadIndex]` for per-thread write heads.
Lock-free via atomic increments. Write order is always indeterministic.

---

## 7. Key Design Decisions

### Why Struct-Based Jobs (Not Classes)

1. **Blittable memcpy**: Value types are bitwise-copyable. No GC tracking, no pointer fixups.
2. **No GC pressure**: Scheduling a million jobs allocates zero managed heap memory.
3. **Cache locality**: Job struct data is compact and contiguous.
4. **Burst compatibility**: Burst cannot handle managed types.

### Why No Closures/Lambdas

Closures in C# are compiler-generated classes (managed heap allocations) that capture
variables by reference. Violates blittability, GC, and Burst constraints.

### Why Copy-On-Schedule Instead of Shared Memory

Each job works on its own private copy of scalar/struct data with zero synchronization cost.
Only NativeContainers (which wrap pointers to shared native memory) require safety tracking,
done at schedule time rather than at every access.

---

## 8. Performance Characteristics

### Scheduling Overhead

Total scheduling overhead: roughly **1-5 microseconds per job** in optimized builds on
modern hardware (post-2022 improvements). In development builds with safety checks,
significantly higher.

### Batch Size Guidelines

For IJobFor/IJobParallelFor:

- **Too large** (batch = total count): No parallelism.
- **Too small** (batch = 1): Atomic operation overhead dominates.
- **Rule of thumb**: Each batch should represent at least **a few microseconds of work**.
  For trivial per-element operations (vector add), batch sizes of 32-256 are typical.
  For expensive per-element operations (raycast, pathfind), batch sizes of 1-4 are fine.

### Job Starvation

If jobs are too small (sub-microsecond execution time), the worker thread wake/sleep cycle
can cost more than the job itself. Mitigation: batch small jobs together, use larger batch
sizes, or combine multiple logical jobs into a single job.

---

## 9. Rust Implementation Considerations

1. **Rust's ownership system replaces AtomicSafetyHandle.** `Send`/`Sync` traits and the
   borrow checker provide compile-time guarantees that Unity must enforce at runtime.

2. **Copy-on-schedule maps to `Send` bounds.** A Rust job struct that is `Send` can be
   safely moved to another thread.

3. **Work stealing crates exist.** `crossbeam-deque` provides Chase-Lev work-stealing
   deques. `rayon` is a production-grade work-stealing scheduler.

4. **The NativeContainer parallel write problem maps to split borrows and atomics.**
   Index-partitioned access is just disjoint `&mut [T]` slices (see `split_at_mut`).

5. **Burst's win comes from LLVM + no-GC + SIMD.** Rust already compiles through LLVM
   with no GC. The main delta is auto-vectorization hints and `std::simd`.

6. **16 KB chunk size for ECS.** If adopting archetype-chunk iteration, 16 KB aligns with
   L1 cache.

7. **Per-thread wake using futex.** `parking_lot` provides portable futex-like primitives.

---

## Sources

- Unity Manual: Jobs overview (6000.0)
- Unity Manual: Job system overview (6000.2)
- Unity Manual: Job dependencies
- Unity Manual: The safety system in the C# Job System (2020.1)
- Unity Manual: NativeContainer (2020.1)
- Unity Blog: Improving job system performance scaling in 2022.2 (parts 1 and 2)
- Unity Discussions: Job system thread waking optimizations in 2022.3.62f1 and 6000.0.48f1
- AtomicSafetyHandle source (UnityCsReference on GitHub)
- Unity Entities package: Use the job system with Entities (1.0)
- Burst User Guide (1.3, 1.8)
- HPC# overview (Burst 1.8)
- Sebastian Schoener: Job Types in the Unity Job System (blog, 2019)
- Sebastian Schoener: The Whipping Boy Approach to Job Scheduling (blog, 2025)
- Jackson Dunstan: Job System Tutorial
- Arm: Arm Neon and the Unity Burst compiler (learn.arm.com)
