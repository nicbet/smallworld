# Unreal Engine 5 Job/Task System Architecture

Technical deep-dive into UE5's threading and task scheduling internals, written for
engine developers building their own job systems.

**Source files** (UE5 source tree):
- `Engine/Source/Runtime/Core/Public/Async/TaskGraphInterfaces.h`
- `Engine/Source/Runtime/Core/Private/Async/TaskGraph.cpp`
- `Engine/Source/Runtime/Core/Public/Async/Fundamental/Scheduler.h`
- `Engine/Source/Runtime/Core/Public/Async/Fundamental/Task.h`
- `Engine/Source/Runtime/Core/Public/Tasks/Task.h`
- `Engine/Source/Runtime/Core/Public/Tasks/Pipe.h`

---

## 1. Architecture Overview

UE5's task system is a **two-layer architecture**:

1. **LowLevelTasks** (`LowLevelTasks::FScheduler` + `LowLevelTasks::FTask`) -- the actual
   backend scheduler and worker thread pool. This is the execution engine.
2. **High-level APIs** -- both the legacy **TaskGraph** (`FTaskGraphInterface`,
   `TGraphTask<T>`) and the modern **UE::Tasks** system (`UE::Tasks::Launch`,
   `UE::Tasks::FPipe`) build DAGs and manage dependencies, then hand individual
   ready-to-run tasks down to `FScheduler`.

Both high-level APIs share the same `FScheduler` backend. This was a deliberate design
choice enabling incremental migration: new code uses `UE::Tasks::Launch`, legacy code
continues working unchanged, and there is only one thread pool.

### Core DAG Execution Model

The task graph is a **directed acyclic graph (DAG)** of tasks connected by
prerequisite/subsequent edges. Each task node tracks its own
`NumberOfPrerequistitesOutstanding` -- an atomic counter. When a predecessor completes,
it decrements this counter on every subsequent; when the counter hits zero, that
subsequent is queued for execution.

This is fully decentralized: no global scheduler lock on the critical path. The only
synchronization is per-task atomic decrements.

---

## 2. Thread Naming and Roles

### Named Threads (ENamedThreads::Type)

UE5 uses a bitmask enum encoding thread identity, queue index, and priority:

```
Bits 0-7:    Thread index
Bit 8:       QueueIndex (0=MainQueue, 1=LocalQueue)
Bit 9:       TaskPriority (0=Normal, 1=High)
Bits 10-11:  ThreadPriority (0=Normal, 1=High, 2=Background)
```

| Thread                   | Role                                                        |
|--------------------------|-------------------------------------------------------------|
| `GameThread`             | All UObject access, Blueprint exec, gameplay logic, GC      |
| `ActualRenderingThread`  | Render commands, scene proxy updates. 1 frame behind game.  |
| `RHIThread`              | GPU command submission. Translates FRHICommand to API calls. |
| `StatsThread`            | Stats collection and aggregation                            |
| `AudioThread`            | Sound queue graph evaluation, spatialization                 |
| `AnyThread` (0xFF)       | Sentinel: "any unnamed worker thread"                       |

Composite values like `GameThread_Local`, `AnyHiPriThreadHiPriTask`,
`AnyBackgroundThreadNormalTask` combine bitmask fields.

### Dual-Queue Named Threads

Each `FNamedTaskThread` has two queues:
- **MainQueue** (`QueueIndex=0`): Auto-executes tasks until empty during the thread's
  processing loop.
- **LocalQueue** (`QueueIndex=1`): Requires manual draining via
  `ProcessThreadUntilIdle()` or `WaitUntilTasksComplete()`.
- Recursion limited to two nesting levels to prevent stack overflow.

### Rendering Pipeline Threading

The three rendering threads form a **frame pipeline**:
- **Game Thread** processes Frame N+1 (logic, transform updates)
- **Render Thread** (frontend) generates platform-agnostic `FRHICommand` structs for Frame N
- **RHI Thread** (backend) translates commands into DX12/Vulkan/Metal API calls for Frame N-1

Rather than parallelizing a single frame's game logic, UE **pipelines across frames**.
The game thread blocks at tick boundaries until the render thread catches up. This
trades one frame of latency for deterministic, race-free inter-system communication.

Communication: `ENQUEUE_RENDER_COMMAND` macro creates a local class with a virtual
`Execute()` function, inserted into the render command queue. `FRenderCommandFence`
provides `BeginFence()` / `Wait()` for game-to-render synchronization.

---

## 3. Worker Thread Pool Design

### Thread Count

`FPlatformMisc::NumberOfWorkerThreadsToSpawn()` returns approximately
**logical cores - 1** (reserving one for the game thread):

| Hardware              | Worker Threads |
|-----------------------|----------------|
| 8P / 16L cores       | 7              |
| 12P / 20L cores      | 11             |
| 6P / 12L cores       | 5              |
| Dedicated server      | 1              |

Overridable via `-workersthreadpool X` command-line argument.

### Thread Pools

UE5 maintains multiple specialized pools:

| Pool                            | Priority               | Typical Count | Purpose                     |
|---------------------------------|------------------------|---------------|-----------------------------|
| `GThreadPool`                   | `TPri_SlightlyBelowNormal` | ~14       | General-purpose async work  |
| `GBackgroundPriorityThreadPool` | `TPri_Lowest`          | 2             | Low-priority background     |
| `GLargeThreadPool`              | `TPri_Normal`          | ~14           | Editor-only (lighting builds) |
| `GIOThreadPool`                 | varies                 | varies        | I/O operations              |

### Task Graph Thread Groups (Priority-Based)

| Group               | Thread Priority         | Count    |
|----------------------|-------------------------|----------|
| `TaskGraphThreadHP`  | `SlightlyBelowNormal`  | Per-core |
| `TaskGraphThreadNP`  | `BelowNormal`          | ~`NumberOfCores()` |
| `TaskGraphThreadBP`  | `Lowest`               | Variable |

### Thread Affinity

- **Windows/consoles**: explicit core pinning via OS APIs
- **Android**: `__NR_sched_setaffinity` for explicit core binding
- **macOS/iOS**: `thread_policy_set()` for affinity *sets* (not direct binding)
- `FRunnableThread::Create()` accepts an `InThreadAffinityMask` parameter

`FQueuedThreadPoolBase` manages `TArray<IQueuedWork*> QueuedWork` (pending) and
`TArray<FQueuedThread*> QueuedThreads` (idle workers). Created via
`FQueuedThreadPool::Allocate()` then `Create(NumThreads, StackSize=128KB, Priority)`.
Initialized in `FEngineLoop::PreInit()`.

---

## 4. Task Definitions and Granularity

### Pattern 1: TGraphTask (Task Graph DAG Tasks)

The most structured pattern. User defines a task class, the system manages DAG execution:

```cpp
class FMyGraphTask
{
public:
    FMyGraphTask(/* args */) {}

    static ESubsequentsMode::Type GetSubsequentsMode()
        { return ESubsequentsMode::TrackSubsequents; }

    ENamedThreads::Type GetDesiredThread()
        { return ENamedThreads::AnyThread; }

    TStatId GetStatId() const
        { RETURN_QUICK_DECLARE_CYCLE_STAT(FMyGraphTask, STATGROUP_TaskGraphTasks); }

    void DoTask(ENamedThreads::Type CurrentThread,
                const FGraphEventRef& CompletionEvent)
    {
        // Work here.
        // Can call CompletionEvent->DontCompleteUntil(SubTask)
        // to defer completion until sub-tasks finish.
    }
};

// Launch with prerequisites:
FGraphEventRef Event = TGraphTask<FMyGraphTask>::CreateTask(
        &Prerequisites, ENamedThreads::GameThread)
    .ConstructAndDispatchWhenReady(/* constructor args */);
```

**Core class hierarchy:**
- `FBaseGraphTask` -- base, tracks `NumberOfPrerequistitesOutstanding` (atomic counter)
- `TGraphTask<TTask>` -- template wrapper, inherits `TConcurrentLinearObject` + `FBaseGraphTask`
- `FGraphEventImpl` -- manual synchronization events

**Required task interface:**
- `GetStatId()` -- returns `TStatId` for cycle stat tracking
- `GetDesiredThread()` -- returns `ENamedThreads::Type`
- `GetSubsequentsMode()` -- `TrackSubsequents` or `FireAndForget`
- `DoTask(CurrentThread, CompletionEvent)` -- the actual work

### Pattern 2: FAsyncTask / FAutoDeleteAsyncTask (Thread Pool)

Simpler, for independent work items without DAG dependencies:

```cpp
class FMyWork : public FNonAbandonableTask
{
    friend class FAsyncTask<FMyWork>;

    FMyWork(/* args */) { ... }
    void DoWork() { /* actual work */ }

    FORCEINLINE TStatId GetStatId() const {
        RETURN_QUICK_DECLARE_CYCLE_STAT(FMyWork, STATGROUP_ThreadPoolAsyncTasks);
    }
};

// Manual lifetime (reusable):
auto* Task = new FAsyncTask<FMyWork>(args);
Task->StartBackgroundTask(GThreadPool);
Task->EnsureCompletion();  // blocks until done
delete Task;

// Fire-and-forget (self-deletes):
(new FAutoDeleteAsyncTask<FMyWork>(args))->StartBackgroundTask();
```

Both implement `IQueuedWork` (methods `DoThreadedWork()` and `Abandon()`).

### Pattern 3: UE::Tasks::Launch (Modern, UE 5.0+)

Lambda-friendly, minimal boilerplate:

```cpp
using namespace UE::Tasks;

// Fire-and-forget:
Launch(UE_SOURCE_LOCATION, []{ DoExpensiveWork(); });

// With return value:
TTask<int> T = Launch(UE_SOURCE_LOCATION, []{ return ComputeValue(); });
int Result = T.GetResult();  // blocks until complete

// With prerequisites:
FTask A = Launch(UE_SOURCE_LOCATION, []{ Step1(); });
FTask B = Launch(UE_SOURCE_LOCATION, []{ Step2(); }, Prerequisites(A));
```

### Pattern 4: IQueuedWork (Raw Interface)

Minimal contract for direct thread pool submission:

```cpp
virtual void DoThreadedWork() = 0;
virtual void Abandon() = 0;
```

### Task Granularity

Typical task sizes observed in UE5 engine code:
- **Coarse**: physics substep, animation evaluation, AI navigation mesh update (~1-10ms)
- **Medium**: per-primitive visibility, per-mesh LOD calculation (~0.1-1ms)
- **Fine**: `ParallelFor` batches for particle updates, vertex processing (~10-100us)

The `ParallelFor` `MinBatchSize` parameter controls the floor: iterations are not split
below this threshold to avoid task overhead dominating work.

---

## 5. ParallelFor

### Signatures

```cpp
// Basic:
void ParallelFor(int32 Num, TFunctionRef<void(int32)> Body,
    EParallelForFlags Flags = None);

// With batch size control:
void ParallelFor(const TCHAR* DebugName, int32 Num, int32 MinBatchSize,
    TFunctionRef<void(int32)> Body, EParallelForFlags Flags = None);
```

### Flags

| Flag                   | Effect                                                       |
|------------------------|--------------------------------------------------------------|
| `None`                 | Default balanced static distribution                         |
| `ForceSingleThread`    | Execute sequentially on calling thread                       |
| `Unbalanced`           | Task-graph-based dynamic distribution for variable-cost work |
| `PumpRenderingThread`  | Process render commands while waiting                        |
| `BackgroundPriority`   | Run on background-priority threads                           |

### Implementation

`ParallelForImpl::ParallelForInternal` divides `Num` iterations into batches controlled
by `MinBatchSize`. Each batch becomes a task graph task dispatched to workers. The
calling thread participates, executing one batch inline. The `Unbalanced` flag switches
from static partitioning to dynamic task-graph-based distribution.

---

## 6. Scheduling and Work Stealing

### FScheduler

The `LowLevelTasks::FScheduler` singleton manages the worker pool:

- Worker threads run `WorkerMain()` loops calling `TryExecuteTaskFrom()`
- Uses `TLocalQueueRegistry` with per-worker local deques (size ~1024)
- `FQueuedLowLevelThreadPool` is a compatibility adapter running `IQueuedWork` on `FScheduler`

### Queue Preference

When launching a low-level task:
- `EQueuePreference::Local`: If called from a worker, places the task directly on that
  thread's local queue (LIFO for cache locality)
- Otherwise: enqueued to a shared queue for any worker to pick up

### Work Stealing (Chase-Lev)

Per-thread deques follow the classic **Chase-Lev work-stealing** pattern:
- **Owner** pushes/pops from the "top" (LIFO -- cache-warm, depth-first)
- **Thieves** steal from the "bottom" (FIFO -- coarsest-grained work, breadth-first)
- When a worker's deque empties, it randomly selects a victim and attempts to steal
- This naturally balances load without global contention

### Priority Levels (LowLevelTasks::ETaskPriority)

| Priority           | Usage                    |
|--------------------|--------------------------|
| `High`             | Time-critical tasks      |
| `Normal`           | Default                  |
| `BackgroundHigh`   | Background, higher urgency |
| `BackgroundNormal` | Background, default      |
| `BackgroundLow`    | Lowest priority          |

### Task State Machine (LowLevelTasks::ETaskState)

Bit-flag state machine for lock-free lifecycle management:

```
Base flags:  Ready | ScheduledFlag | CanceledFlag | RunningFlag |
             ExpeditingFlag | ExpeditedFlag | CompletedFlag

Combined:    Scheduled       = Ready | ScheduledFlag
             CanceledAndReady = Ready | CanceledFlag
             Running         = Ready | ScheduledFlag | RunningFlag
             ...
```

Atomic CAS transitions enable lock-free task lifecycle without mutexes.

---

## 7. Dependencies

### FGraphEvent

A reference-counted object (`FGraphEventRef` = `TRefCountPtr<FGraphEvent>`) representing
a completion signal.

**Key members:**
- List of `FBaseGraphTask*` subsequents (tasks waiting on this event)
- Atomically closeable: once `DispatchSubsequents()` fires, no new subsequents can be added

**Lifecycle:**
1. Created automatically when a `TGraphTask` is constructed (`GetCompletionEvent()`),
   or manually via `FBaseGraphTask::CreateGraphEvent()`
2. Other tasks add themselves as subsequents
3. When the owning task's `DoTask()` completes, `DispatchSubsequents()`:
   - Atomically grabs and closes the subsequent list
   - For each subsequent, decrements its `NumberOfPrerequistitesOutstanding`
   - If the decrement reaches zero, that task is queued for execution
4. `DontCompleteUntil(FGraphEventRef)` allows a task to defer its own completion until
   a sub-task finishes

### UE::Tasks Dependencies

```cpp
// Single prerequisite:
FTask B = Launch(UE_SOURCE_LOCATION, []{}, Prerequisite);

// Multiple prerequisites:
FTask C = Launch(UE_SOURCE_LOCATION, []{}, Prerequisites(A, B));

// Nested tasks (parent waits for child):
FTask Parent = Launch(UE_SOURCE_LOCATION, []{
    FTask Child = Launch(UE_SOURCE_LOCATION, []{});
    AddNested(Child);  // Parent completion deferred until Child finishes
});
```

### Task Retraction

When `Wait()` is called and the waited-on task hasn't started yet, the waiting thread
can **retract** it and execute it inline, recursively retracting prerequisites. This
prevents deadlock where all workers block waiting on tasks no one is executing.

---

## 8. Thread Safety Primitives

### Mutexes

| Type             | Description                                              |
|------------------|----------------------------------------------------------|
| `FCriticalSection` | Platform mutex (WinAPI `CRITICAL_SECTION`, pthread)    |
| `FScopeLock`     | RAII wrapper -- locks in ctor, releases in dtor          |
| `FRWLock`        | Multiple-reader / exclusive-writer. **Not recursive.**   |
| `FReadScopeLock` | RAII read lock for `FRWLock`                             |
| `FWriteScopeLock`| RAII write lock for `FRWLock`                            |

### Events

| Type         | Description                                                   |
|--------------|---------------------------------------------------------------|
| `FEvent`     | OS signaling primitive. One waits, another triggers.          |
| `FEventRef`  | RAII wrapper for `FEvent*`                                    |
| `FTaskEvent` | Task-system-aware signal. Does not block worker threads.      |

### Atomics

`TAtomic<T>` is UE's own wrapper but is **deprecated** -- Epic recommends
`std::atomic<T>` directly. Used extensively in task graph internals
(e.g., `NumberOfPrerequistitesOutstanding` decrement).

### Lock-Free Data Structures

| Type                           | Pattern | Notes                                 |
|--------------------------------|---------|---------------------------------------|
| `TQueue<T>`                    | MPSC or SPSC (template param `EQueueMode`) | Unbounded linked list. MPSC uses atomic CAS on enqueue. |
| `TMpscQueue<T>`                | Multi-producer, single-consumer | Based on Dmitry Vyukov's 1024cores.net algorithm |
| `TSpscQueue<T>`                | Single-producer, single-consumer | 1024cores.net. Recycles consumed nodes. |
| `TLockFreePointerListFIFO<T>`  | Lock-free FIFO | Pointer-based lock-free list |
| `TLockFreePointerListLIFO<T>`  | Lock-free LIFO (stack) | Used for internal free-lists |

---

## 9. UE5-Specific Improvements over UE4

### UE::Tasks System (UE 5.0+)

The modern replacement for raw TaskGraph usage. Key improvements:

- **Dramatically less boilerplate** -- no need to define a full task class
- **Lambda-friendly** -- `UE::Tasks::Launch(UE_SOURCE_LOCATION, []{})`
- **Return values** -- `TTask<int> T = Launch(..., []{ return 42; }); T.GetResult();`
- **FPipe** for serialized execution chains (green threads)
- **FTaskEvent** for manual synchronization without task bodies
- **Nested tasks** (`AddNested()`) for completion dependencies without blocking
- **Built-in debug names** on all constructs via `UE_SOURCE_LOCATION`

### FPipe (Task Pipes / Green Threads)

- Lightweight, non-copyable, non-movable
- Construction allocates no dynamic memory
- Tasks on the same pipe execute **sequentially** (FIFO if no prerequisites), but may
  migrate between worker threads
- Use case: serialized access to a thread-unsafe resource without a mutex
- Avoids mutex hazards: no priority inversion, no deadlock
- API: `HasWork()`, `WaitUntilEmpty()`, `IsInContext()`

### Oversubscription (UE 5.5+)

Replaces deprecated `BusyWait()`. When a worker thread blocks on `Wait()`, the
scheduler activates a **standby thread** to keep the core busy. The standby thread
parks itself when the blocked thread resumes.

This is automatic -- most wait functions are already instrumented. Visible in Unreal
Insights as "oversubscription scopes." Eliminates CPU waste from spin-waiting while
preventing throughput collapse when workers block.

### Task Tracing (Unreal Insights)

Six lifecycle stages traced per task: Created, Launched, Scheduled, Started, Finished,
Completed. Relationship arrows show prerequisites, subsequents, and nested tasks.
Critical path analysis identifies the longest execution chain in the DAG.

---

## 10. Key Design Decisions and Rationale

1. **Shared backend, layered APIs** -- Both TaskGraph and UE::Tasks share
   `LowLevelTasks::FScheduler`. One thread pool, incremental migration path.

2. **Named threads for deterministic subsystem ordering** -- Game, render, RHI, audio,
   stats threads are pinned. Guarantees subsystem isolation (UObject access only on game
   thread, RHI calls only on render/RHI thread) without per-call locking.

3. **Frame pipelining over parallel simulation** -- Game(N+1) | Render(N) | RHI/GPU(N-1).
   Trades one frame of latency for deterministic, race-free inter-system communication.

4. **Atomic prerequisite counting over centralized scheduling** -- Each task tracks its
   own outstanding prerequisite count. Completion is decentralized: no global lock on
   the critical path.

5. **Chase-Lev work stealing** -- Per-thread deques with LIFO push/pop (cache locality)
   and FIFO stealing (coarse-grained). Balances work without global contention.

6. **Task retraction to prevent deadlock** -- When a thread `Wait()`s on an unstarted
   task, it can retract and execute it inline (recursively retracting prerequisites).
   Prevents all-workers-blocked scenarios.

7. **Green thread pipes over mutexes** -- `FPipe` gives sequential semantics with
   parallel throughput, avoiding priority inversion and deadlock hazards.

8. **Oversubscription over busy-waiting** (UE 5.5) -- Standby threads activate during
   waits instead of spinning. Eliminates CPU waste while maintaining throughput.

9. **Dual-queue named threads** -- MainQueue (auto-drained) + LocalQueue (manually
   drained) lets systems enqueue high-priority work for immediate processing during
   explicit pump calls while background work accumulates.

10. **Lock-free structures from 1024cores.net** -- `TMpscQueue` and `TSpscQueue` are
    direct implementations of Dmitry Vyukov's algorithms, chosen for minimal overhead
    and formal correctness guarantees.

---

## Relevance to a Rust Job System

Key takeaways for a Rust engine developer:

- **Two-layer design is worth emulating**: a low-level scheduler (thread pool + work
  stealing) with higher-level DAG/pipe abstractions built on top. In Rust, the low
  level could use `crossbeam`'s deque or a custom Chase-Lev implementation.

- **Atomic prerequisite counting** is simple and scales well. Rust's `AtomicU32` with
  `fetch_sub(1, Release)` + `fence(Acquire)` on zero maps directly.

- **Task retraction** is critical for avoiding deadlock in systems where worker threads
  may `wait()` on other tasks. Without it, a fixed-size pool can deadlock when all
  workers are waiting.

- **Named/pinned threads** for GPU submission and main-thread-only work are essential.
  In Rust, a dedicated `std::thread` with a `crossbeam::channel` receiver works.

- **FPipe** maps to a Rust `mpsc::channel` feeding a single consumer task that runs
  sequentially -- or a tokio `Mutex<()>` guarding serialized async blocks.

- **Oversubscription** (standby threads) is harder in Rust without runtime support.
  Consider `rayon`'s `scope` + `yield_now`, or implement standby threads manually
  using `parking_lot::Condvar`.

---

## Sources

- [UE Source Explained - Thread Architecture](https://github.com/donaldwuid/unreal_source_explained/blob/master/main/thread.md)
- [Tasks Systems in Unreal Engine - Official Docs](https://dev.epicgames.com/documentation/unreal-engine/tasks-systems-in-unreal-engine)
- [Tasks System References - Official Docs](https://dev.epicgames.com/documentation/unreal-engine/tasks-system-references-in-unreal-engine)
- [Task Graph Insights - Official Docs](https://dev.epicgames.com/documentation/unreal-engine/task-graph-insights-in-unreal-engine-5)
- [Threaded Rendering - Official Docs](https://dev.epicgames.com/documentation/unreal-engine/threaded-rendering-in-unreal-engine)
- [Parallel Rendering Overview - Official Docs](https://dev.epicgames.com/documentation/unreal-engine/parallel-rendering-overview-for-unreal-engine)
- [FBaseGraphTask API Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Core/FBaseGraphTask)
- [FScheduler API Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Core/Async/Fundamental/FScheduler)
- [ENamedThreads::Type API Reference](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Core/ENamedThreads__Type)
- [ParallelFor API Reference](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Core/ParallelFor)
- [Thread Pools - Gamedev Guide (ikrima)](https://ikrima.dev/ue4guide/engine-programming/threading-model/)
- [Multithreading Codex (jdelezenne)](https://jdelezenne.github.io/Codex/UE4/Multithreading.html)
- [Multi-Threading: Task Graph System - Community Wiki](https://unrealcommunity.wiki/multi-threading:-task-graph-system-pah8k101)
- [TMpscQueue API Reference](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Core/TMpscQueue)
