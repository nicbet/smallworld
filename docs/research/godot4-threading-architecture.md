# Godot 4 Threading & Work System Architecture

Technical deep-dive into Godot 4's threading internals for engine developers.
Focused on C++ implementation, not GDScript usage.

---

## 1. Architecture -- WorkerThreadPool

### Design Overview

Godot 4's `WorkerThreadPool` (authored primarily by Pedro J. Estébanez / "RandomShaper")
is a global singleton defined in `core/object/worker_thread_pool.h/.cpp`. It replaces
Godot 3's ad-hoc threading model where each subsystem (rendering, physics, resource
loading) spawned its own threads independently, leading to unbounded thread counts.

The pool centralizes **all** background work -- rendering server, physics server, resource
loading, and user tasks -- into a single fixed-size thread pool. Key design goals:

- Stable, bounded thread count at all times
- HTML5/Web threading support (SharedArrayBuffer + pthreads)
- Prevent thread-count explosion from independent subsystems

### Godot 3 vs Godot 4

| Aspect | Godot 3 | Godot 4 |
|--------|---------|---------|
| Scene processing | Single-threaded | Still single-threaded for mutations; ProcessGroups for `_process`/`_physics_process` |
| Server threading | Optional, ad-hoc `Thread` objects | Centralized WorkerThreadPool |
| Thread count | Unbounded (each subsystem spawns as needed) | Fixed pool, bounded at init |
| Resource loading | Single-threaded or manual threading | Multi-threaded via pool with collaborative waiting |
| Physics | Optional separate thread | Runs as pump task in pool |
| Rendering | Optional separate thread | Runs as pump task in pool, CommandQueueMT syncs via yield |
| Web/HTML5 | Largely single-threaded | Pool-aware with configurable pthread sizing |

### Scheduling Model

**Not work-stealing.** The pool uses a **centralized FIFO queue with two priority tiers**,
protected by a single `BinaryMutex`, with per-thread condition variables for wake-up. This
is a deliberate simplicity choice -- the pool serves heterogeneous workloads (rendering,
physics, resource loading, user tasks) and prioritizes correctness and deadlock avoidance
over maximum throughput on embarrassingly-parallel workloads.

### Thread Pool Sizing

```cpp
void init(int p_thread_count, float p_low_priority_ratio);
```

**Thread count formula** (native platforms):
```
thread_count = OS::get_singleton()->get_processor_count() - 1
```

Logical cores (including hyperthreads) minus one, reserving one core for the main thread.

**Low-priority thread cap:**
```
max_low_priority_threads = clamp(thread_count * low_priority_ratio, 1, thread_count - 1)
```

Reserves at least one thread exclusively for high-priority work, preventing low-priority
tasks (resource loading) from starving rendering or physics servers.

On Web/WASM, thread count was historically hardcoded to 1 (causing deadlocks with physics
group tasks). Fixed in PR #104458 to query the emscripten pthread pool size at runtime,
with configurable export settings (`godot_pool_size` default 4, `emscripten_pool_size`
default 8).

---

## 2. Key Data Structures

### Task

```cpp
struct Task {
    TaskID self;                          // unique ID, monotonically incrementing
    Callable callable;                    // GDScript/bound method path
    void (*native_func)(void *);          // C++ function pointer (standalone)
    void (*native_group_func)(void *, uint32_t);  // C++ indexed callback (group)
    void *native_func_userdata;
    String description;                   // debug label
    Semaphore done_semaphore;             // signaled on completion
    bool completed;
    bool pending_notify_yield_over;
    bool is_pump_task;                    // daemon/server task flag
    Group *group;                         // non-null if part of a group
    SelfList<Task> task_elem;             // intrusive list node
    uint32_t waiting_pool;               // count of pool threads waiting on this
    uint32_t waiting_user;               // count of external threads waiting on this
    bool low_priority;
    BaseTemplateUserdata *template_userdata;  // type-erased C++ callback
    int pool_thread_index;               // which thread is running this (-1 if none)
};
```

`SelfList<Task>` is Godot's intrusive doubly-linked list -- O(1) insert/remove, no heap
allocation for the list node (lives inside the Task).

### Group

```cpp
struct Group {
    GroupID self;
    SafeNumeric<uint32_t> index;           // atomic work counter (next index to claim)
    SafeNumeric<uint32_t> completed_index;  // atomic count of finished indices
    uint32_t max;                           // total elements to process
    Semaphore done_semaphore;
    SafeFlag completed;                     // set when all elements done
    SafeNumeric<uint32_t> finished;         // finished task count (for cleanup)
    uint32_t tasks_used;                    // how many Task objects were allocated
};
```

`SafeNumeric<T>` wraps `std::atomic<T>` with Godot's API. `SafeFlag` wraps
`std::atomic<bool>`. The `index` field is the core load-balancing mechanism -- each worker
atomically increments it to claim work items, so distribution is naturally balanced without
explicit partitioning.

### ThreadData

```cpp
struct ThreadData {
    uint32_t index;
    Thread thread;                // OS thread handle
    bool signaled;                // prevent duplicate wake-ups
    bool yield_is_over;           // resume from yield
    bool pre_exited_languages;
    bool exited_languages;
    bool has_pump_task;           // running a daemon/server task
    Task *current_task;           // what this thread is executing
    Task *awaited_task;           // what this thread is blocked waiting for
    ConditionVariable cond_var;   // per-thread wake signal
};
```

### Memory Allocation

Tasks and Groups are allocated from `PagedAllocator<Task, false, TASKS_PAGE_SIZE>` and
`PagedAllocator<Group, false, GROUPS_PAGE_SIZE>`:

- `TASKS_PAGE_SIZE = 1024`
- `GROUPS_PAGE_SIZE = 256`

`PagedAllocator` is Godot's slab allocator -- allocates pages of N objects and hands them
out. The `false` template parameter means it is **not** thread-safe internally, so all
allocations happen under `task_mutex`. Known pain point (proposal #11201): tasks are never
freed until explicitly waited on -- no fire-and-forget; every task must be joined via
`wait_for_task_completion()` to reclaim memory.

---

## 3. Task Submission & Execution

### add_task / add_native_task

Allocates a `Task` from `task_allocator`, assigns a monotonically incrementing `TaskID`,
populates the callable/native function pointer, and calls `_post_tasks()`.

### _post_tasks(Task**, uint32_t count, bool is_pump)

Under `task_mutex`:

1. If no threads exist (single-threaded mode), executes tasks synchronously and returns.
2. For each task, appends to the appropriate `SelfList`:
   - High-priority: `task_queue`
   - Low-priority: if `low_priority_threads_used < max_low_priority_threads`, goes to
     `task_queue` and increments counter; otherwise parked in `low_priority_task_queue`
3. For pump tasks: if `pump_task_count >= thread_count`, may spawn additional threads.
4. Calls `_notify_threads(count)` to wake workers.

### _notify_threads(uint32_t count)

Two-pass notification under `task_mutex`:

**Pass 1:** Iterate `threads[]` for idle threads (`!signaled && current_task == nullptr`).
Set `signaled = true`, call `cond_var.notify_one()`.

**Pass 2:** If insufficient idle threads found, notify threads currently executing but
collaboratively waiting (`awaited_task` is set), so they pick up new work via
`_wait_collaboratively`.

**Known scalability issue** (GitHub #103933): On high-core-count machines (24-core Ryzen),
notification iterates all N threads to find waiters -- O(N) work per task completion. The
fix is to maintain a separate registry of waiting threads.

### Worker Loop (_thread_function)

```
loop {
    lock(task_mutex);
    _handle_runlevel();           // check for shutdown transitions

    if (task_queue empty && low_priority_task_queue empty) {
        thread.signaled = false;
        thread.cond_var.wait(lock); // sleep until notified
        continue;
    }

    Task *task = task_queue.first();  // dequeue from high-priority first
    if (!task) {
        task = low_priority_task_queue.first();  // then low-priority
    }
    task_queue.remove(task);
    thread.current_task = task;

    unlock(task_mutex);
    _process_task(task);
    lock(task_mutex);

    thread.current_task = nullptr;
    // cleanup, promotion, re-loop
}
```

Strict FIFO within each priority tier. No work-stealing between threads.

### Task Processing (_process_task)

1. Sets `thread_safe_for_nodes = false` (scene tree access never safe from pool threads).
2. Initializes `ScriptServer` for this thread if needed (GDScript VM per-thread setup).
3. **Group tasks:** Atomically increments `group->index` to claim a work item. If
   `index < group->max`, executes the callback. Loops claiming more indices until
   exhausted. When `completed_index` hits `max`, posts `group->done_semaphore`.
4. **Standalone tasks:** Executes the function pointer or Callable.
5. Sets `task->completed = true`.
6. Wakes waiters via semaphore/condvar.
7. Calls `_try_promote_low_priority_task()`.

### Low-Priority Promotion

When a task completes and a low-priority slot opens:

```
if (low_priority_task_queue not empty && low_priority_threads_used < max) {
    task = low_priority_task_queue.first();
    low_priority_task_queue.remove(task);
    task_queue.add(task);           // promote to main queue
    low_priority_threads_used++;
    _notify_threads(1);
}
```

Aging/promotion mechanism prevents indefinite starvation of low-priority tasks.

---

## 4. Collaborative Waiting & Deadlock Prevention

The most sophisticated part of the design. When a pool thread must wait for another task
(e.g., resource loading depends on another resource), simply blocking would reduce
parallelism and risk deadlock if all threads block on each other.

### wait_for_task_completion(TaskID)

**External threads** (not pool workers): blocks on `task->done_semaphore.wait()`.

**Pool threads:**
- Returns `ERR_BUSY` if the target task is deeper in the call stack (prevents recursive
  deadlock -- if task A waits for task B, but B is below A on the same thread's stack, it
  can never make progress).
- Otherwise calls `_wait_collaboratively(task)`.

### _wait_collaboratively(Task *target)

```
loop {
    lock(task_mutex);
    if (target->completed) break;

    thread.awaited_task = target;

    if (task_queue has tasks) {
        Task *other = task_queue.first();
        task_queue.remove(other);
        _unlock_unlockable_mutexes();   // release up to MAX_UNLOCKABLE_LOCKS=2 locks
        _process_task(other);           // execute on THIS thread's stack frame
        _lock_unlockable_mutexes();     // re-acquire
        continue;
    }

    // No work available -- actually block
    thread.cond_var.wait(lock);
}
thread.awaited_task = nullptr;
```

**Unlockable mutexes** (`MAX_UNLOCKABLE_LOCKS = 2`): The collaborative waiter may hold
mutexes that other tasks need. The pool tracks up to 2 "unlockable" mutexes per thread in
a thread-local array. Before processing a borrowed task, it releases these locks, then
re-acquires after. Critical for the resource loading system, which holds `ResourceLoader`
mutexes.

---

## 5. Pump Tasks (Daemon/Server Tasks)

Long-lived tasks used by engine servers (rendering, physics). They run indefinitely,
yielding periodically to let the pool reclaim the thread for other work.

- `is_pump_task = true` on the Task
- `has_pump_task = true` on the ThreadData
- Pool tracks `pump_task_count`; if it equals or exceeds `thread_count`, more threads may
  be spawned to prevent starvation
- Pump tasks call `yield()` to voluntarily pause
- `notify_yield_over()` signals the pump task to resume
- Yield functions are kept private/internal to prevent accidental daemon tasks

The rendering server's `CommandQueueMT` synchronizes through this yield mechanism -- the
render thread yields, the main thread flushes commands, then signals yield-over to resume.

---

## 6. Thread Safety Primitives

### Mutex

`MutexImpl<std::recursive_mutex>` (recursive, default) and `BinaryMutex` wrapping
`std::mutex` (non-recursive). All methods are `const` with `mutable` underlying mutex.
`MutexLock` is `[[nodiscard]]` RAII with `temp_unlock()`/`temp_relock()` for condition
variable patterns.

`SafeBinaryMutex<Tag>` achieves recursive semantics on a non-recursive mutex via
thread-local lock counting -- cheaper than `std::recursive_mutex`.

### RWLock

Thin wrapper over `std::shared_mutex`. RAII wrappers `RWLockRead`/`RWLockWrite`.

### Semaphore

Hand-rolled from `std::mutex` + `std::condition_variable` (not platform semaphore).
`wait()` has spurious wakeup protection.

### SpinLock

Three variants:
- **Apple:** `os_unfair_lock` (kernel-assisted hybrid with priority donation)
- **Generic:** TTAS (test-and-test-and-set) pattern with `AtomicBool`
- All padded to **128-byte** cache lines

Used only for sub-microsecond critical sections (ObjectDB, RID tables).
`_cpu_pause()` dispatches per-arch: x86 `_mm_pause`, ARM `yield`, PPC `or 27,27,27`,
RISC-V encoded pause.

### SafeNumeric / SafeFlag

`std::atomic<T>` wrappers with **uniform acquire-release ordering everywhere** -- no
relaxed operations exposed. Deliberate simplicity-over-performance choice to eliminate
ordering bugs.

### Thread

`std::thread` wrapper. Thread IDs are engine-assigned monotonic `uint64_t` (not OS TIDs).
Main thread is always ID 1. `make_main_thread()` uses atomic exchange for exactly-once
guarantee.

---

## 7. The Server Architecture

### Pattern

Godot servers are **singleton daemons** implementing the **mediator pattern**. Callers
never hold pointers to internal objects -- they receive opaque `RID` (Resource ID) handles
and interact through the server's public API. Servers own all data, process it on dedicated
threads, and return results through the same facade.

This is fundamentally a **share-nothing / message-passing** design. Each server is
effectively an **actor** processing messages sequentially.

### CommandQueueMT -- Core Cross-Thread Mechanism

**File:** `core/templates/command_queue_mt.h`

Backbone of RenderingServer and PhysicsServer threading:

- **Linear append buffer** (`LocalVector<uint8_t>`, default 64 KB) -- not a circular
  buffer. After flushing, the buffer clears entirely and capacity is reused.
- **Synchronization:** `BinaryMutex` guards mutations, `ConditionVariable` blocks sync
  callers, `std::atomic<bool> pending` provides lock-free emptiness checks,
  `sync_head`/`sync_tail` counters track sync progress.
- **Command encoding:** Template structs (`Command<T,M,NeedsSync,Args...>`) use
  `std::decay_t<Args>` in a `Tuple`, unpacked at call time via `IndexSequence` expansion.
  Each command is 8-byte aligned with a `uint64_t` size prefix. Max command size: 1024
  bytes (copied to stack during execution).
- **Three push modes:**
  - `push()` -- async, fire-and-forget
  - `push_and_sync()` -- blocks until execution
  - `push_and_ret()` -- blocks and captures return value
- **Flush loop:** Copies each command to a stack-local buffer, **unlocks the mutex during
  `call()`**, then relocks. For sync commands, increments `sync_head` and notifies waiters.

**Known contention issue:** The entire `_draw()` frame render is a single queued command.
While the mutex is released during execution, background threads pushing commands (e.g.,
glTF loading) contend on the same `BinaryMutex`, causing ~30x slower asset loading with
the `Separate` thread model. Proposed fix: double-buffered pending/flushing queues.

### RenderingServer Threading

**Files:** `servers/rendering/rendering_server_default.h/.cpp`

- **RSG subsystem decomposition:** 14 static pointers to subsystems (`texture_storage`,
  `mesh_storage`, `material_storage`, `light_storage`, `particles_storage`, `gi`, `fog`,
  `canvas_render`, `rasterizer`, `canvas`, `viewport`, `scene`, etc.). Backends access via
  `RSG::` macro -- swappable between GLES3 and RenderingDevice.
- **FUNC macros** (`FUNC1`, `FUNC3`, `FUNCRID`, etc.) generate thread-aware wrappers: if
  `Thread::get_caller_id() != server_thread`, push to CommandQueueMT; otherwise execute
  immediately.
- **Thread loop:** Runs as a WorkerThreadPool pump task. Acquires GL context, then loops:
  `yield()` -> `flush_all()` -> repeat until exit.
- **Frame lifecycle:** Main thread pushes `_draw(swap, dt)`. Render thread executes:
  `begin_frame` -> XR sync -> scene culling -> particle update -> `draw_viewports` ->
  canvas render -> `end_frame`. Main thread can overlap physics/game logic for the next
  frame.

### RenderingDevice & Render Graph (Godot 4.3+)

**Files:** `servers/rendering/rendering_device.h`, `rendering_device_graph.h/.cpp`

Commands are **not** sent immediately to the GPU. They're serialized into a `uint8` vector
and become nodes in a **DAG** (~300 nodes/frame):

- Each resource embeds an `RDG::ResourceTracker`. The graph analyzes read/write
  dependencies to build edges automatically.
- **Topological sort** identifies "levels" of commands that can execute in parallel.
  Barriers are inserted between levels, not individual commands. Commands within a level
  are sorted by type (copy/draw/compute) to minimize GPU pipeline switches.
- Results: 60-80% reduction in `vkCmdPipelineBarrier` calls, ~10% frametime improvement
  (NVIDIA), 40%+ on particle-heavy scenes, <1% CPU cost for graph construction.

### PhysicsServer Threading (WrapMT Pattern)

**File:** `servers/physics_3d/physics_server_3d_wrap_mt.h`

Proxy class wrapping a real `PhysicsServer3D` (GodotPhysics or Jolt) with its own
`CommandQueueMT`. Same FUNC macro system routes calls through the queue when called from
non-physics threads.

**Direct-access escapes:** `space_get_direct_state()` and `body_get_direct_state()` bypass
the queue with a `Thread::is_main_thread()` check. `body_test_motion()` originally
bypassed too but caused BVH race conditions (duplicate nodes from concurrent shape
updates) -- fixed by routing through the queue (PR #72491).

### AudioServer Threading (Lock-Free)

AudioServer is unique -- always on a dedicated thread (audio driver's mix callback), using
a **lock-free linked list** (`SafeList`) instead of `CommandQueueMT`:

- `SafeList`: Atomic CAS-based singly-linked list. `insert()` prepends via
  `compare_exchange_strong` loop. `erase()` moves nodes to a "graveyard" list. Actual
  deallocation deferred until `active_iterator_count == 0`.
- All atomics use `memory_order_seq_cst`.

### NavigationServer Threading (Deferred Sync)

Uses a **deferred synchronization phase** -- changes to maps, regions, and agents are
batched and applied before/after the physics frame:

- `RWLock` on mesh data enables concurrent pathfinding reads during baking writes
- Async baking runs on `WorkerThreadPool`
- Agent avoidance (RVO2) resolved in batch during the sync window

---

## 8. Scene Tree Threading

### Thread Guards

Godot 4.1 introduced three tiers of **debug-only** guard macros in `node.h`:

- `ERR_THREAD_GUARD` -- caller must be in the same ProcessGroup or on main thread
- `ERR_MAIN_THREAD_GUARD` -- must be on main thread (used for `add_child`,
  `remove_child`, etc.)
- `ERR_READ_THREAD_GUARD` -- any thread within any group can read, but raw workers blocked

The underlying check uses a **thread-local boolean** (`current_thread_safe_for_nodes`) set
to `true` only on the main thread at startup. Guards are compiled out in release builds --
**zero enforcement in production**. ~80 guard invocations in `node.cpp`.

The scene tree has a single coarse `Mutex` (from `_THREAD_SAFE_CLASS_` macro) protecting
group membership bookkeeping. **No per-node locking.** Safety is cooperative, not locked.

### ProcessGroups (Godot 4.1, PR #75901)

Nodes opt into `MAIN_THREAD`, `SUB_THREAD`, or `INHERIT`. Sub-thread groups fan out to
WorkerThreadPool. A `thread_local Node* current_process_thread_group` tracks which group
each worker is processing. Benchmark: ~3x speedup for ~100 animated characters.

### call_deferred / MessageQueue

**CallQueue** uses a paged linear allocator (4KB pages). Messages are placement-new'd into
page byte arrays as `Message { Callable, type, args_or_notification }` followed by inline
`Variant` arguments.

**Thread safety:** A regular `Mutex` protects the main queue. Thread-local queue instances
skip locking entirely.

**Flush behavior:** The mutex is **unlocked during each individual message execution**,
allowing re-entrant enqueues. A `flushing` boolean prevents recursive flush. The queue
flushes **5+ times per frame** across physics iterations and process steps.

**Three deferred channels:**

1. `call_deferred()` -- pushes to global MessageQueue, always executes on main thread
2. `call_deferred_thread_group()` -- pushes to per-ProcessGroup CallQueue, executes on
   that group's thread
3. `call_thread_safe()` -- adaptive: calls directly if same thread group, otherwise
   defers to group queue

---

## 9. Shutdown Sequence

Three-phase runlevel transition:

1. **PRE_EXIT_LANGUAGES:** Signal threads to prepare for shutdown. Wait for all threads
   to become idle.
2. **EXIT_LANGUAGES:** Each thread calls `ScriptServer::thread_exit()` to tear down
   per-thread GDScript VM state.
3. **EXIT:** `finish()` joins all OS threads, deallocates remaining tasks/groups from
   PagedAllocators.

---

## 10. Known Limitations & Bottlenecks

### Scene Tree

The scene tree is fundamentally **single-threaded for structural mutations**.
`add_child()`/`remove_child()` remain main-thread-only. ProcessGroups help parallelize
`_process`/`_physics_process` calls but structural changes still serialize. Thread guards
are diagnostic-only in debug -- zero protection in release builds.

### Server Internal Parallelism

Servers are **internally single-threaded** -- they can run on their own thread but cannot
parallelize work *within* that thread. No parallel broadphase, no parallel command
recording. Each server is effectively a single-consumer queue.

### CommandQueueMT Contention

Single `BinaryMutex` protects both queuing and flushing, causing ~30x slowdowns for
background asset loading with the `Separate` render thread model (GitHub #112452).

### WorkerThreadPool Scalability

Notification loop iterates all N threads per task completion -- O(N) work per task on
high-core-count machines (GitHub #103933). No work-stealing means load imbalance on
heterogeneous workloads.

### GDScript Threading

GDScript has no GIL but also no thread safety. RefCounted cache-line contention makes
32-thread code run at **0.5x** single-thread speed (GitHub #117640).

### Memory

Tasks are never freed until explicitly waited on (`wait_for_task_completion`). No
fire-and-forget. Every task must be joined to reclaim memory (proposal #11201).

---

## 11. Key Design Decisions & Philosophy

Juan Linietsky (reduz) explicitly rejected a full job system because "usability would end
up severely affected." The architecture is a textbook **share-nothing / message-passing**
design:

- Scene tree (user-facing) is single-threaded
- Servers (engine internals) communicate via unidirectional command queues with opaque RID
  handles
- Each server is effectively an actor processing messages sequentially

**Comparison with other engines:**

| | Godot 4 | Unity DOTS | Unreal |
|-|---------|------------|--------|
| Model | Server actors + centralized pool | Full job system + ECS | Task graph + dedicated threads |
| Parallelism | Moderate (inter-server) | High (intra-system) | High (render thread 1 frame behind) |
| Complexity | Low | Very high | High |
| Perf ceiling | Lower (single-consumer servers) | 10-100x potential | High (mature pipeline) |

Godot chose the **simplest correct design** -- correctness and usability over maximum
parallelism. The server/RID pattern eliminates shared mutable state almost entirely. The
single global mutex on the thread pool is a known tradeoff: correctness with collaborative
waiting, pump tasks, and priority promotion under a single lock is much easier to reason
about than lock-free alternatives.

---

## 12. Implications for a Rust Job System

### What maps well to Rust

- **Server/RID pattern -> channels + SlotMap arenas.** The server thread owns data, the
  game thread holds `Key<T>` handles. No `Arc<Mutex<T>>` needed. Godot's proven
  architecture validates this approach.
- **Command queues -> `crossbeam::channel` or `flume`.** Sending commands through channels
  is idiomatic Rust and matches Godot's CommandQueueMT pattern.
- **Group tasks with atomic index claiming** is elegant for data-parallel work. Natural
  load balancing via `fetch_add`, minimal contention. Maps directly to
  `AtomicU32::fetch_add(Ordering::AcqRel)` in Rust.
- **Two-tier priority with promotion** is simple and effective. Low-priority overflow queue
  with aging avoids starvation without complex priority queues.

### Where to diverge

- **Work-stealing** instead of FIFO. Godot chose FIFO because it simplifies collaborative
  waiting and pump tasks. For a voxel engine with homogeneous workloads (streaming,
  meshing, LOD), a work-stealing deque (Rayon/Tokio style) may be more appropriate.
- **Per-queue sharding** instead of single global mutex. If targeting 16+ cores, Godot's
  O(N) notification and single-lock contention will not scale. Consider sharded queues or
  lock-free structures.
- **Arena allocation with epoch-based reclamation** (crossbeam-epoch) instead of
  PagedAllocator with mandatory join. Eliminates the fire-and-forget problem.
- **Apple's `os_unfair_lock`** for SpinLock is worth noting -- it's a kernel-assisted
  hybrid with priority donation, maps to `parking_lot` internals on macOS. Consider
  `parking_lot::Mutex` for Rust equivalent.
- **Uniform acquire-release ordering** on all atomics is a simplicity choice worth
  considering -- it trades some theoretical performance for eliminating ordering bugs. In
  Rust, `Ordering::AcqRel` everywhere is the equivalent.

### The real parallelism wins

The real gains come from **server-layer threading** (GPU commands, physics broadphase,
asset streaming), not scene graph parallelism. Godot's 3x ceiling for ProcessGroups
confirms this. For smallworld, the server/RID pattern with per-server threads and a
work-stealing pool for data-parallel tasks (brick streaming, SVO construction, LOD
selection) is the right architecture.

---

## Sources

- `core/object/worker_thread_pool.h/.cpp` -- Pool implementation
- `core/templates/command_queue_mt.h` -- Command queue
- `core/os/mutex.h`, `semaphore.h`, `spin_lock.h`, `thread.h` -- Primitives
- `servers/rendering/rendering_server_default.h/.cpp` -- Render server threading
- `servers/rendering/rendering_device_graph.h/.cpp` -- Render graph DAG
- `servers/physics_3d/physics_server_3d_wrap_mt.h` -- Physics WrapMT
- `core/templates/safe_list.h` -- Lock-free list for AudioServer
- `scene/main/node.h` -- Thread guard macros
- GitHub issues: #103933 (scalability), #112452 (CommandQueueMT contention),
  #117640 (GDScript RefCounted contention)
- GitHub PRs: #75901 (ProcessGroups), #90268 (server threads via pool),
  #104458 (Web pthread fix), #72491 (physics thread safety)
- GitHub proposals: #11201 (task memory)
