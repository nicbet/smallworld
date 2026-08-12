# Architectural Foundations

Before a single triangle reaches the screen or a physics tick advances, a game engine must answer three architectural questions that will shape every subsystem built on top of it:

1. How does simulation talk to rendering?
2. How is game state represented and composed?
3. Who owns what data on which thread?

Get any of these wrong and the consequences compound — tangled coupling, data races, cache-hostile memory layouts, and an ever-growing surface of subtle bugs that ship to players.

This chapter establishes the foundational design philosophy of the Smallworld engine. We begin with the **Game–Render Firewall**, the hard boundary that decouples simulation logic from GPU presentation, drawing heavily on Unreal Engine 5's proven Game Thread / Render Thread split while adapting it to Rust's ownership semantics and wgpu's internally-synchronized device model. We then examine **Data-Driven Design**, contrasting it with the deep inheritance hierarchies of traditional object-oriented engines and explaining why plain-data components, opaque handles, and serializable asset descriptors produce more flexible, cache-friendly, and team-scalable codebases. Finally, we explore **Thread Ownership** — the principle that each thread exclusively owns its mutable data and communicates only through owned-value messages sent across channels — and the practical architecture of split work-stealing pools that keep game simulation and rendering from stepping on each other.

Together, these three pillars — the firewall, data-driven composition, and thread ownership — form the contract that every subsequent chapter builds upon. Understanding them is prerequisite to understanding anything else in this engine.

## What You Will Learn in This Chapter {.unnumbered}
- **The Game–Render Firewall:** Why simulation code must never see a GPU resource, how the extract step bridges the two worlds through a delta-driven `FramePacket`, and what the narrow invariant actually protects.
- **Data-Driven Design vs. Object-Oriented Hierarchies:** The architectural case against `UObject`-style inheritance, virtual dispatch on hot paths, and runtime reflection — and the ECS alternative built from plain-data structs, dense arrays, and trait-based polymorphism.
- **Thread Ownership and CSP Messaging:** How Rust's ownership system replaces locks and shared mutable state with compile-time thread-safety guarantees, and why the engine uses Communicating Sequential Processes (CSP) channels instead of shared-memory concurrency.
- **Work-Stealing Pools:** Why the engine splits its worker pools into game and render tiers, how this prevents priority inversion, and why a unified task-graph scheduler is the planned evolution rather than the starting architecture.
- **Handle-Based Resource Management:** The role of generational indices, `SlotMap` arenas, and opaque handles in providing safe, cache-friendly resource access without garbage collection or reference counting on hot paths.

## The Game–Render Firewall

The single most consequential architectural decision in any multi-threaded game engine is the boundary between simulation and presentation. Get the boundary wrong — let gameplay code touch GPU buffers, or let the renderer reach into the live world — and the result is a system that cannot be parallelized, cannot be profiled in isolation, and cannot evolve without cascading breakage.

### The Problem: Coupling Kills Concurrency

In a naive single-threaded engine, the game loop is a monolithic sequence: read input, update the world, build draw calls, submit to the GPU, present. Every subsystem has implicit access to everything else. The renderer reads entity transforms directly from the world. The world queries GPU readback buffers for occlusion data. Materials are modified in the same function that evaluates AI behavior.

This works — until it doesn't. The moment you need to overlap simulation and rendering across frames (which you do, because a 16.6 ms frame budget leaves no room for serial execution of two complex workloads), every shared data access becomes a synchronization hazard. Locks destroy throughput. Lock-free structures introduce subtle memory-ordering bugs. And the more subsystems you bolt on — physics, animation, audio, streaming — the more tangled the dependency graph becomes.

```
THE COUPLING PROBLEM — NAIVE ENGINE

    ┌──────────────────────────────────────────────────┐
    │                 SINGLE THREAD                     │
    │                                                   │
    │  Input ──► World Update ──► Build Draws ──► GPU   │
    │               │                  ▲                 │
    │               │   direct access  │                 │
    │               ▼                  │                 │
    │         Entity Transforms ◄──────┘                 │
    │         GPU Buffers ◄────────────┘                 │
    │         Material State ◄─────────┘                 │
    │                                                   │
    │  Problem: Everything reads and writes everything.  │
    │  Parallelism is impossible without locks.          │
    └──────────────────────────────────────────────────┘
```

Unreal Engine 5 solves this with a hard architectural split: the **Game Thread** owns the world, and the **Render Thread** owns the GPU. Between them sits an extraction step that copies the data the renderer needs into a form the renderer owns. The two threads then operate independently, overlapped by one frame.

Smallworld adopts this same split — and strengthens it.

### The Firewall Defined

The Game–Render Firewall is not a suggestion or a coding guideline. It is a structural constraint enforced by Rust's type system:

**Game code never sees a `wgpu::Device`, a bind group, or a GPU buffer.** The extract step is the boundary. Everything above it speaks in transforms, materials, and handles. Everything below it speaks in draw commands and GPU resources.

This is Design Principle 3 of the engine, and the narrow invariant it protects is precise: **the Render Thread exclusively owns device-local resources and command submission.** Nothing else in the engine may call `queue.submit()`, allocate a device-local buffer, or write to a render target.

The firewall constrains _game code_, not engine internals. Engine subsystems — the asset pipeline, the staging pool, the streaming coordinator — may create and populate CPU-visible staging resources from any thread, because `wgpu` is internally synchronized and designed for multi-threaded resource creation. What they may not do is touch device-local memory or record render commands. This distinction is critical: it means asset loading and streaming can proceed concurrently without violating the thread-safety contract.

```
┌──────────────────────────────────────────────────────────────────────┐
│                           GAME DOMAIN                                │
│                                                                      │
│   World, Components, Systems, Input, Time, Behaviors, AssetServer    │
│                                                                      │
│   Speaks in: Transforms, Materials, Handles, Events, Commands        │
│   Owns: World state, game logic, gameplay tags, entity hierarchies   │
│   Never touches: wgpu::Device, bind groups, GPU buffers,             │
│                  render targets, command encoders                     │
│                                                                      │
└────────────────────────────────┬─────────────────────────────────────┘
                                 │
                          ═══════╪═══════  EXTRACT (the firewall)
                                 │
                                 │  FramePacket (owned, Send)
                                 │  - Views, Lights, Environment
                                 │  - MeshDrawDelta (upserts/removes)
                                 │  - ResourceOps (upload/free)
                                 │  - Per-backend custom payloads
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                          RENDER DOMAIN                                │
│                                                                      │
│   RenderScene, RenderGraph, GPU Pools, Render Targets, TLAS/BLAS     │
│                                                                      │
│   Speaks in: Draw commands, GPU resources, shader bindings           │
│   Owns: All device-local memory, command submission, swapchain       │
│   Never touches: World, Components, game state                       │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### Why Not Shared Memory?

A reasonable engineer might ask: why not use `Arc<RwLock<T>>` or atomics to share state between threads? Rust makes concurrent access safe — why add the overhead of copying data through channels?

The answer is threefold:

1. **Determinism.** When the Game Thread sends an owned `FramePacket` through a channel, the Render Thread processes a frozen snapshot of the world at a known point in time. There are no races over partially-updated state. The renderer never sees a half-applied physics step or a transform that was modified after the camera was positioned. This is not merely convenient — it is what makes the engine's deterministic simulation guarantee possible.

2. **Profiling and isolation.** Each thread's work can be traced, timed, and optimized in complete isolation. A game-thread stall does not pollute render-thread timings. A GPU bottleneck is visible in render-thread metrics without any game-thread interference. When the two domains share mutable state, profiling becomes archaeology — you are reconstructing causality from interleaved lock acquisitions.

3. **Evolutionary safety.** When the renderer needs a new piece of data — say, a per-entity wind reactivity scalar — the change is surgical: add a field to the appropriate component, add a line to the extract step, add a field to the delta packet. The game side and render side never see each other's changes. When data is shared via `Arc`, adding a field to a shared struct requires auditing every reader and writer across every thread simultaneously. The blast radius of a change is the entire codebase.

In Unreal Engine 5, the same boundary exists — `FSceneProxy` copies data from the game world into render-thread-owned proxies — but it is enforced by convention, documentation, and code review. A determined (or careless) programmer can reach across the boundary. In Rust, the boundary is structural: the `FramePacket` is `Send` and contains no references into the World. Once sent, the Game Thread cannot read it. Once received, the Render Thread cannot reach back. The type system is the enforcement mechanism, not human discipline.

### The Extract Step in Detail

The extract step runs at the end of every game tick, after all systems have finished updating the world. It is the synchronization point between the two-frame overlapped pipeline.

The extract step does three things:

1. **Diffs the World.** The engine maintains a `ChangeTracker` — a set of dirty flags per component type per entity, plus sets of spawned and despawned entities. The extract step walks these dirty sets, not the entire world. A static entity that hasn't moved since frame 1 costs zero extract work on frame 10,000. This is what makes `EntityFlags::STATIC` meaningful — it is not a hint for the renderer; it is a statement that this entity's delta contribution is zero.

2. **Builds a `FramePacket`.** The packet contains everything the Render Thread needs to advance one frame: camera views (with TAA jitter and dynamic resolution scale), light parameters, environment state (sky, fog, wind), resource operations (mesh uploads, texture uploads, material updates, frees), and the scene delta — upserts and removes for the shared mesh draw store, plus per-backend custom payloads.

3. **Sends through a bounded channel.** The packet is an owned value — no references, no borrowed lifetimes. Once sent, the Game Thread is free to mutate the World for the next frame. The bounded channel provides natural backpressure: if the Render Thread falls behind, the Game Thread blocks on send rather than racing ahead and accumulating unbounded latency.

```rust
struct FramePacket {
    frame_index:  u64,
    views:        Vec<ViewParams>,       // main camera + aux views (RTT, probes)
    lights:       Vec<LightParams>,      // small; re-sent in full each frame
    environment:  EnvironmentParams,
    resource_ops: Vec<ResourceOp>,
    mesh_delta:   MeshDrawDelta,         // shared mesh stream updates
    deltas:       Vec<(BackendId, Box<dyn Any + Send>)>,  // per-backend custom
}
```

Every field is owned and `Send`. There are no `Arc`s, no `Rc`s, no raw pointers. The packet is a value that can be moved across a thread boundary with zero runtime cost beyond the channel send itself. This is a consequence of Rust's ownership model — a value with no references can be moved to any thread without synchronization.

### Delta-Driven vs. Snapshot-Driven Extraction

An important distinction from simpler engines: Smallworld's extraction is **delta-driven**, not snapshot-driven. The Render Thread maintains a persistent **`RenderScene`** — a retained representation of the current draw state — and the `FramePacket` carries only the changes since the last frame.

```
DELTA-DRIVEN EXTRACTION

  Frame N:   [ Spawn Entity A ]  [ Spawn Entity B ]  [ Spawn Entity C ]
             FramePacket: { upsert A, upsert B, upsert C }

  Frame N+1: [ Move Entity B ]
             FramePacket: { instance_upsert B }   ← only the change

  Frame N+2: [ Despawn Entity A ]
             FramePacket: { remove A }

  Frame N+3: [ No changes ]
             FramePacket: { }   ← empty delta; A, B, C state persists
                                  in the retained RenderScene
```

This is the same retained-scene principle as UE5's proxy system. The difference is the transport: in UE5, proxies write into shared memory structures that the render thread reads. In Smallworld, deltas cross the boundary as owned values through a channel. No shared ownership, no lifetime entanglement.

The practical payoff is enormous for static-heavy scenes. An open world with 50,000 placed rocks and trees costs almost nothing to extract once the initial spawns are sent — only moving, spawning, or despawning entities generate delta traffic. This is what makes large persistent worlds viable without extract becoming the frame bottleneck.

### The Feedback Path

The pipeline is not one-directional. The Render Thread sends a `FrameFeedback` back to the Game Thread after each frame is submitted, through a separate channel. The Game Thread typically reads feedback from frame N−2 while processing frame N. Never a synchronous wait.

```
time ──────────────────────────────────────────────────────▶

Game:    │ update(N)          │ update(N+1)          │ update(N+2)
         │ reads feedback(N-2)│ reads feedback(N-1)  │ reads feedback(N)
         │           send ──┐ │            send ──┐  │
         │                  │ │                   │  │
Render:  │ render(N-1)      │ │ render(N)         │  │ render(N+1)
         │ feedback(N-1) ───┘ │ feedback(N) ──────┘  │
```

Feedback data has two ages. **CPU-side data** — cull statistics, visible mesh counts — describes the frame the feedback was sent after. **GPU-derived data** — timestamps, compute readbacks — is older: at submit time the GPU has not yet executed the frame, so query results are collected through a **frames-in-flight readback ring** (2–3 buffered query sets, polled via `map_async` without blocking) and stamped with the frame they actually measure.

```rust
struct FrameFeedback {
    frame_index:    u64,
    gpu_time:       Option<GpuTimingFeedback>,   // None until first results land
    occlusion:      OcclusionFeedback,
    readback:       Vec<ReadbackResult>,
}
```

Because feedback is always from a past frame, game code must treat it as **advisory** — useful for adaptive quality (drop LOD if GPU is overloaded), streaming priority (don't stream what's culled), and profiling, but never as ground truth for the current frame's state. This is a deliberate design constraint, not a limitation: treating stale data as advisory eliminates an entire class of frame-timing bugs where game logic makes decisions based on render state that has already changed.

### Historical Context: How Other Engines Handle the Boundary

Understanding why the firewall matters requires understanding how the industry arrived here:

| Engine                  | Boundary Model                                                                                                                   | Trade-off                                                                                                         |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| **Early Unreal (UE3)**  | Monolithic loop; renderer reads `AActor` state directly                                                                          | Simple, but impossible to overlap threads                                                                         |
| **Unity (pre-DOTS)**    | Main thread does everything; `CommandBuffer` defers GPU work                                                                    | Single-threaded bottleneck; renderer cannot run ahead                                                             |
| **Unreal Engine 5**     | Game Thread / Render Thread split via `FSceneProxy` extraction; optional RHI Thread                                              | Proven at AAA scale; boundary enforced by convention                                                               |
| **Godot 4**             | Server model — `RenderingServer` receives commands from scene tree; dedicated render thread in 4.3+                              | Clean separation, but command-based (no retained delta)                                                           |
| **Smallworld**          | Rust ownership-enforced firewall; delta-driven `FramePacket` via bounded channel; retained `RenderScene`; no shared mutable state | Strictest boundary; type-system enforcement; zero extract cost for static entities; one frame of input latency     |

The key insight from UE5's architecture — which Smallworld inherits wholesale — is that one frame of input latency (~16 ms at 60 fps) is an acceptable trade for up to double the throughput when both threads carry comparable load. The simulation runs ahead by one frame; the renderer draws the previous frame's state. The user perceives smoothness at the cost of a single frame of reaction delay — a trade-off that every modern competitive and cinematic game has accepted.

### Common Pitfalls in Game–Render Boundaries
- **Pitfall 1: Leaking GPU types into game code.**
  _The Danger:_ Exposing `wgpu::Buffer` or `wgpu::Texture` to gameplay systems. Once a game programmer discovers they can read a depth buffer to implement line-of-sight, the abstraction is permanently breached — every future refactor must preserve that access.
  _The Fix:_ Opaque handles (`AssetHandle<T>`, `ResourceHandle<T>`) are the only types game code ever holds. GPU resource lifetime is engine-managed. Gameplay queries (raycasting, visibility) go through CPU-side spatial structures, never GPU readbacks.
- **Pitfall 2: Snapshot extraction on every frame.**
  _The Danger:_ Re-sending the entire world state to the renderer each frame. In a scene with 100,000 entities, this means copying transforms, materials, and flags for every entity even when 99% haven't changed.
  _The Fix:_ Delta-driven extraction with a `ChangeTracker`. Static entities cost zero after their first frame. Only dirty components generate packets.
- **Pitfall 3: Synchronous GPU readbacks.**
  _The Danger:_ Blocking the game thread to read GPU query results (occlusion counts, timestamps). This serializes the pipeline and negates the benefit of the two-thread split.
  _The Fix:_ Asynchronous feedback through a readback ring. Results arrive 2–3 frames late and are treated as advisory, never authoritative.

## Data-Driven Design vs. Object-Oriented Hierarchies

With the thread boundary established, we turn to the second foundational question: how is game state represented? The answer determines everything from cache performance to team workflow to the engine's ability to serialize, replicate, and inspect game worlds.

### The Object-Oriented Trap

The traditional answer, established by Unreal Engine's `UObject` system and Unity's original `MonoBehaviour` model, is deep inheritance:

```
THE OOP INHERITANCE PYRAMID

                        UObject
                           │
                        AActor
                       /       \
               APawn            ALight
              /      \
       ACharacter    AVehicle
          │
     APlayerCharacter
```

This hierarchy encodes behavior through virtual dispatch. An `APlayerCharacter` inherits movement from `ACharacter`, which inherits collision from `APawn`, which inherits lifecycle from `AActor`, which inherits serialization and reflection from `UObject`. Each layer adds virtual methods, member variables, and invariants that all descendants must respect.

The problems with this approach are well-documented and severe:

1. **The Diamond Problem and Fragile Base Classes.** Adding a new capability (say, "swimmable") requires modifying the hierarchy. Do you create `ASwimmableCharacter` as a new subclass? What about `ASwimmableVehicle`? Every new axis of behavior multiplies the hierarchy. And modifying a base class — say, changing `AActor`'s tick order — risks breaking every class below it.

2. **Cache-Hostile Memory Layout.** Objects in an inheritance hierarchy are typically heap-allocated individually. Iterating over 10,000 enemies means chasing 10,000 pointers to scattered heap locations. Each pointer dereference is a potential L1 cache miss — at ~100 cycles per miss on modern hardware, this turns a 1 ms loop into a 10 ms stall.

3. **Virtual Dispatch on Hot Paths.** Every virtual method call requires an indirect jump through a vtable pointer. On a hot path executed 100,000 times per frame — say, updating transforms — the indirect branch prediction misses add measurable overhead. More importantly, virtual calls defeat inlining, preventing the compiler from optimizing across call boundaries.

4. **Runtime Reflection Overhead.** UE5's `UPROPERTY` macro system generates reflection metadata for every field of every `UObject` subclass. This enables the editor, serialization, garbage collection, and Blueprint access. It is also thousands of lines of generated code per class, longer compile times, and a runtime cost for GC scanning even on objects that don't need it.

5. **Merge Conflicts and Team Scaling.** When behavior is encoded in class hierarchies, two programmers working on different features often need to modify the same base class. Binary assets (UE5's `.uasset` files) make this worse — they cannot be meaningfully diffed or merged.

```
HISTORICAL ENGINE PARADIGMS

+------------------------------------+----------------------------------------------+
| 1. Deep Inheritance (UE3, Unity)   | Behavior = base classes + virtual dispatch.   |
|                                    | Heap-allocated objects. Cache-hostile.        |
+------------------------------------+----------------------------------------------+
| 2. Component-Object (UE4/5)        | Components attach to Actors. Better, but      |
|                                    | UObject overhead remains. Reflection system.  |
+------------------------------------+----------------------------------------------+
| 3. Pure ECS (Bevy, Flecs)          | Entity = ID. Component = data. System = fn.   |
|                                    | Cache-friendly dense arrays. No inheritance.  |
+------------------------------------+----------------------------------------------+
| 4. Hybrid Data-Driven (Smallworld) | ECS for the game World. Dense side structs    |
|                                    | for engine internals. Trait objects only where |
|                                    | polymorphism is load-bearing (behaviors, I/O). |
+------------------------------------+----------------------------------------------+
```

### The ECS Alternative: Components as Plain Data

Smallworld's answer is Design Principle 1: **Data-driven. Components are plain data structs. Systems are functions that operate on component stores. No inheritance hierarchies, no virtual dispatch on hot paths.**

An entity is an opaque ID — a generational index into a `SlotMap`. It has no type, no base class, no vtable. What makes a "player" different from a "tree" is not inheritance but the set of components attached:

```rust
// A player: transform + mesh + physics + audio + behavior
let player = world.spawn();
world.add(player, Transform { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE });
world.add(player, MeshRenderer { mesh: player_mesh, material: player_mat, cast_shadows: true, ..default() });
world.add(player, RigidBody { mass: 80.0, drag: 0.1, ..default() });
world.add(player, AudioListener);

// A light: transform + light source. That's it.
let sun = world.spawn();
world.add(sun, Transform { position: DVec3::new(0.0, 1000.0, 0.0), ..default() });
world.add(sun, LightSource { kind: LightKind::Directional { direction: Vec3::NEG_Y, cascade_count: 4 },
                              color: Vec3::ONE, intensity: 10.0, cast_shadow: true, shadow_bias: 0.001 });
```

This is the same modularity as UE5's component attachment — snapping capabilities together to build complex entities — but without the `UObject` machinery. No reflection macros, no garbage collector, no `UPROPERTY` decorators. The type system provides safety; `Send + Sync + 'static` is the only trait bound on components.

#### Why Plain Data Matters

The "plain data" constraint is not aesthetic — it is load-bearing:

- **Dense storage.** Components of the same type are stored in contiguous arrays. Iterating over every `Transform` in the world is a linear scan through packed memory — the CPU prefetcher's dream. Compare this to chasing pointers through an inheritance hierarchy, where each object's `Transform` lives at a different heap address.

- **Trivial serialization.** A plain data struct with no pointers, no trait objects, and no closures can be serialized with `serde` derive macros. An entity's full state is the union of its serializable components. No custom `SaveGame` overrides, no "forgot to serialize that new field" bugs.

- **Safe extraction.** The extract step borrows `&World` — a read-only reference. Because components are data (not behaviors that might call back into the world), read-only access is guaranteed to be side-effect-free. This is what makes the firewall compositionally sound: extraction cannot trigger game logic.

- **Deterministic change tracking.** When game code calls `world.get_mut::<Transform>(entity)`, the returned `&mut Transform` automatically marks the component dirty in the `ChangeTracker`. Because components are inert data, there is no possibility of a mutation happening through a side channel — every change is visible to the tracker.

### The Engine/Game Component Split

Smallworld draws a clean line between **engine components** (defined and consumed by the engine) and **game components** (defined by each game project). Engine components like `Transform`, `MeshRenderer`, `LightSource`, and `RigidBody` have semantic meaning to the engine — the extract step knows how to convert a `MeshRenderer` into a `MeshDrawCommand`, and the physics system knows how to simulate a `RigidBody`. Game components are opaque to the engine:

```rust
// Game-defined components — the engine doesn't know about these
struct Health { current: f32, max: f32 }
struct Inventory { slots: Vec<Option<ItemId>>, capacity: usize }

// Attach them exactly like engine components
world.add(player, Health { current: 100.0, max: 100.0 });
world.add(player, Inventory { slots: vec![None; 20], capacity: 20 });
```

The engine iterates over its known component types during the LATE phase (hierarchy propagation, bounds recomputation, streaming demand) and the EXTRACT phase. Game components participate only through game-registered systems. This split is what keeps the engine's inner loops tight — the physics system iterates over `(RigidBody, Collider, Transform)` tuples, not over arbitrary component sets.

### Asset Descriptors: Data-Driven Entity Archetypes

UE5 uses Data-Only Blueprints to avoid hardcoding asset paths in C++. Smallworld's equivalent is **asset descriptors** — serializable data files (RON, JSON, or a custom binary format) that describe entity archetypes:

```rust
// A game defines its entity archetypes as data, not code
struct EnemyDescriptor {
    mesh:       AssetPath,       // "meshes/goblin.glb"
    material:   AssetPath,       // "materials/goblin.ron"
    health:     f32,
    speed:      f32,
    loot_table: AssetPath,
}
```

The engine loads these descriptors, resolves asset paths to handles, and spawns entities with the appropriate components. Artists and designers edit the data files; programmers define the descriptor schemas and the systems that process them. Because the files are text-based (RON or JSON), they diff and merge cleanly in version control — unlike UE5's binary `.uasset` files, which are effectively opaque to Git.

This is the data-driven principle in action: the _structure_ of a goblin (what components it has, what mesh it uses, how much health it starts with) is data. The _behavior_ of a goblin (how it patrols, how it attacks) is code — either a Rust `Behavior` implementation or a Lua script. The two concerns are cleanly separated, and either can change independently.

### Handle-Based Resources

The fourth design principle completes the data-driven picture: **Handle-based resources. Games hold opaque handles. Lifetime, caching, and GPU upload are engine-managed.**

```rust
struct AssetHandle<T> {
    id:         AssetId,
    generation: u32,
    _marker:    PhantomData<T>,
}

struct ResourceHandle<T> {
    id:         ResourceId,
    generation: u32,
    _marker:    PhantomData<T>,
}
```

- **`AssetHandle<T>`** references an immutable, shared asset (mesh geometry, texture pixels, audio clips). Many entities can hold the same handle. The asset's GPU-resident representation is managed by the engine's GPU pools — game code never knows whether the asset is in VRAM, in the staging pool, or still loading from disk.

- **`ResourceHandle<T>`** references a mutable resource (materials). The game can modify these at runtime — animate a material's emissive intensity, swap a texture — and the `ChangeTracker` records the mutation for the next extract.

Both use **generational indices** for use-after-free detection. When a resource is freed, its slot's generation counter increments. Any handle still holding the old generation will fail a generation check on access — a clean miss, not a use-after-free crash. This provides the safety guarantees of reference counting without the runtime cost of atomic increment/decrement on every copy.

### Pro-Tips for Data-Driven Engine Design
- **Tip 1: Keep Components Under 128 Bytes.** Large components (e.g., an `Inventory` with a heap-allocated `Vec`) break the cache-locality advantage of dense storage. If a component needs variable-size data, store it behind a handle or in a separate side table, and keep the component itself a fixed-size index into that table.
- **Tip 2: Separate Hot and Cold Data.** A `MeshRenderer` has hot data (the transform, which changes every frame for moving objects) and cold data (the mesh handle, which rarely changes). Storing them in the same component means every transform update pays the cache cost of loading the mesh handle. Advanced ECS designs split these into separate components — Smallworld does this implicitly through `Transform` (hot) and `MeshRenderer` (cold) being separate components.
- **Tip 3: Use Gameplay Tags for Cross-System Communication.** When multiple systems need to react to the same state (e.g., "this entity is burning"), avoid coupling them through shared components or direct function calls. Smallworld's hierarchical `GameplayTag` system — inspired by UE5 — lets a fire ability apply `status.debuff.burning`, the damage system query for `status.debuff.*`, and the VFX system spawn particles for the same tag, all without any system knowing about the others.

## Thread Ownership and the Work-Stealing Pools

The firewall establishes the boundary between two threads. But a modern game engine is not two threads — it is a constellation of execution contexts, each with precise ownership boundaries and communication protocols. Getting the threading model wrong produces either data races (shared mutable state) or deadlocks (overly conservative locking). Smallworld's approach eliminates both categories by construction.

### The Ownership Principle

Design Principle 2 states: **Thread ownership. Each thread owns its data exclusively. Communication between threads happens via owned value-typed packets sent through channels — never shared mutable state.**

This principle has a critical refinement: sharing _immutable_ data across threads (`Arc` payloads, mapped staging regions) is permitted. The rule forbids shared **mutability**, not sharing. A mesh's vertex data, once loaded, can be referenced from any thread via `Arc<MeshAsset>`. What no thread may do is mutate that data while another thread holds a reference.

In C++ engines, this distinction is enforced by code review and (hopefully) thread sanitizers. In Rust, it is enforced by the compiler. The `Send` trait marks types that can be moved to another thread. The `Sync` trait marks types that can be referenced from multiple threads simultaneously. `Rc<T>` (reference-counted, not atomic) is neither `Send` nor `Sync` — the compiler will refuse to compile code that shares it across threads. `Arc<T>` is both, because its reference count is atomic. `&mut T` is `Send` but not `Sync` — you can transfer exclusive access, but you cannot share it. These are not runtime checks; they are compile-time guarantees with zero runtime cost.

The practical consequence: **Smallworld has no mutexes on any hot path.** No `RwLock` around the world. No `Mutex` around the render scene. No atomic compare-and-swap loops for resource management. Each thread owns its data outright, and communication happens through bounded channels that transfer ownership.

### The Execution Contexts

The engine uses six execution contexts, each with clear ownership boundaries:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        THREAD OWNERSHIP MAP                            │
├───────────────────────┬────────────────────────┬───────────────────────┤
│    THREAD             │    OWNS                │    COMMUNICATES VIA   │
├───────────────────────┼────────────────────────┼───────────────────────┤
│ Game Thread (main)    │ World, Input, Time,    │ Sends FramePacket     │
│                       │ Systems, AssetServer   │ to Render Thread;     │
│                       │                        │ receives FrameFeedback│
├───────────────────────┼────────────────────────┼───────────────────────┤
│ Render Thread         │ GpuContext,            │ Receives FramePacket; │
│ (dedicated)           │ RenderScene, Pools,    │ sends FrameFeedback   │
│                       │ RenderGraph, Targets   │ back                  │
├───────────────────────┼────────────────────────┼───────────────────────┤
│ Game Worker Pool      │ Nothing persistent —   │ Scoped tasks via      │
│ (work-stealing)       │ borrows work items     │ join() / parallel_for │
├───────────────────────┼────────────────────────┼───────────────────────┤
│ Render Worker Pool    │ Nothing persistent —   │ Scoped tasks via      │
│ (work-stealing)       │ borrows work items     │ join() / parallel_for │
├───────────────────────┼────────────────────────┼───────────────────────┤
│ Streaming Coordinator │ Demand priority queue, │ Demand from Game;     │
│ (dedicated)           │ budget arbiter         │ UploadBatch to Render │
├───────────────────────┼────────────────────────┼───────────────────────┤
│ Audio Thread          │ Mixer, voices,         │ Drains AudioCommands  │
│ (dedicated)           │ output stream          │ each frame            │
├───────────────────────┼────────────────────────┼───────────────────────┤
│ IO Pool               │ Nothing persistent     │ Tasks from AssetServer│
│ (blocking IO+decode)  │                        │ and Streaming         │
└───────────────────────┴────────────────────────┴───────────────────────┘
```

Several design choices deserve attention:

**Dedicated threads for latency-sensitive work.** The Game Thread, Render Thread, Audio Thread, and Streaming Coordinator each get dedicated OS threads. This prevents them from being preempted by lower-priority work. A physics computation on a shared pool should never cause a present-miss on the render thread.

**Worker pools own nothing.** The game and render worker pools are stateless — they borrow work items from their parent thread, execute tasks, and return results. They hold no persistent state of their own. This makes them safe to use with scoped tasks (`join()`, `parallel_for()`) where the parent thread guarantees the data outlives the task.

**The IO pool is separate.** Blocking filesystem reads and decompression run on a dedicated pool with blocking-friendly threads. This prevents a disk stall from starving CPU-bound game or render work.

### Why Two Worker Pools?

The split between the game worker pool and the render worker pool is a deliberate choice to prevent **priority inversion**:

```
PRIORITY INVERSION — THE SINGLE-POOL PROBLEM

  [ Game Worker Pool (shared) ]
  ┌─────────────────────────────────────────┐
  │ Thread 1: Physics island solve (3 ms)   │
  │ Thread 2: Physics island solve (3 ms)   │
  │ Thread 3: Physics island solve (3 ms)   │
  │ Thread 4: Physics island solve (3 ms)   │  ← All workers busy with physics
  │                                         │
  │     Frustum culling (URGENT, render-    │
  │     critical) QUEUED behind physics     │  ← Frame miss!
  └─────────────────────────────────────────┘

  THE SOLUTION: SPLIT POOLS

  [ Game Worker Pool ]              [ Render Worker Pool ]
  ┌────────────────────────┐       ┌────────────────────────┐
  │ Thread 1: Physics      │       │ Thread 1: Frustum cull │
  │ Thread 2: Physics      │       │ Thread 2: Draw sorting │
  │ Thread 3: Animation    │       │ Thread 3: Batch merge  │
  │ Thread 4: Streaming    │       │                        │
  └────────────────────────┘       └────────────────────────┘
                                   ↑ Never blocked by game work
```

The **game pool** runs physics (the provider's internal parallelism binds here), animation sampling, and streaming demand computation. The **render pool** runs frustum culling, draw call sorting, and batch generation. Because the two-frame pipeline keeps both pools concurrently busy — the game pool works on frame N+1 while the render pool works on frame N — the split costs little utilization. And render-critical culling work never queues behind a heavy physics island solve.

### CSP: Communicating Sequential Processes

The inter-thread communication model is **Communicating Sequential Processes (CSP)** — the same model used by Go's goroutines and channels, by Erlang's actor mailboxes, and by Tony Hoare's original 1978 formalization. Each thread is a sequential process that communicates by sending and receiving messages on typed channels. No shared mutable state. No locks.

In Smallworld, this manifests as:

- **Game → Render:** `FramePacket` via a bounded channel. Game → Render lifecycle events (`Resized`, `Quit`, `DeviceLost`) via a separate out-of-band control channel.
- **Render → Game:** `FrameFeedback` via a separate channel.
- **Game → Audio:** `AudioCommands` drained each frame.
- **Game → Streaming:** Demand requests via a demand channel. Streaming → Render: `UploadBatch` via an upload channel.

Each channel carries owned values. Once a value is sent, the sender no longer has access to it. Once received, the receiver has exclusive ownership. This is the CSP guarantee: processes are isolated; communication is explicit; there is no hidden shared state.

```
CSP MESSAGE FLOW

  Game Thread ─── FramePacket ──────────► Render Thread
       ▲                                       │
       │                                       │
       └──── FrameFeedback ◄───────────────────┘

  Game Thread ─── AudioCommands ────────► Audio Thread

  Game Thread ─── Demand ───────────────► Streaming Coordinator
                                               │
                                               │
  Render Thread ◄── UploadBatch ───────────────┘
```

The control channel deserves special mention. Lifecycle events — window resize, device loss, shutdown — ride an **out-of-band control channel**, separate from the data plane (`FramePacket`). This follows Gregory's control/data-plane separation principle: control must be deliverable independent of packet flow. A paused game that generates no packets still resizes. A stalled pipeline that can't accept packets still shuts down.

### The Staging Pool: Where Threads Meet Safely

There is one place where the strict "no shared mutable state" rule bends: the **staging pool**. Asset IO/decode threads write decoded asset data directly into mapped CPU-visible `wgpu` buffers. The Render Thread later records GPU copies from these buffers into device-local pools.

This is permissible because:

1. **wgpu is internally synchronized.** Creating and mapping buffers from any thread is designed-for usage. The `Device` and `Queue` handles are `Send + Sync`.

2. **Write regions are exclusive.** Each staging region is allocated to exactly one writer. Two decode threads never write to the same region. The write is sequential (write-combined memory); once complete, a `StagingRef` is sent through the `ResourceOp` channel, transferring read access to the Render Thread.

3. **Fence reclamation.** The region is not returned to the pool until the GPU has completed the copy — tracked by the submission fence. No thread can read, write, or reallocate the region while the GPU is using it.

```rust
struct StagingRef {
    buffer: StagingBufferId,
    offset: u64,
    size:   u64,
}
```

The staging pool is engine-internal machinery. Game code never sees it. The firewall constraint applies to game code; the staging pool is part of the engine's infrastructure that sits below the firewall, safely managed by the engine's own threading invariants.

### Budget-Explicit Design

Design Principle 5 rounds out the threading and resource model: **Budget-explicit. Frame time, GPU memory, upload bandwidth, and streaming distance are explicit budgets with engine arbitration, not emergent properties.**

Every pool — GPU meshes, GPU textures, staging buffers, deformed-vertex output, froxel volumes, the GI clipmap — has a named memory budget. When a budget is exceeded, the engine makes an explicit decision: evict the least-recently-used entry, lower LOD, or reject the allocation. Performance is never a mystery; it is an engineering parameter.

This is the antithesis of the "allocate and hope" approach where GPU memory grows unbounded until the driver OOMs, or where streaming distance is tuned by feel until a QA tester finds the pop-in sweet spot. In Smallworld, every budget is a number in a configuration file, visible in the profiler, and subject to the game's quality-scaling logic.

```rust
struct MemoryBudget {
    limit_bytes: u64,
    used_bytes:  u64,
    high_water:  u64,   // peak usage for profiling
}

// Every GPU pool carries its budget
struct GpuMeshPool {
    meshes: SlotMap<GpuId, GpuMesh>,
    budget: MemoryBudget,
}
```

### How Rust Enables This Architecture

It is worth pausing to appreciate that this threading model — no locks, no shared mutable state, compile-time enforcement — is not merely a philosophical preference. It is enabled by specific language features that do not exist in C++ or C#:

| Feature                      | What It Prevents                                                                                         |
| ---------------------------- | -------------------------------------------------------------------------------------------------------- |
| **Ownership + move semantics** | Sending a `FramePacket` through a channel moves it. The sender cannot access it afterward. No aliasing. |
| **`Send` / `Sync` traits**    | The compiler verifies that types crossing thread boundaries are safe to do so. `Rc<T>` won't compile in a `Send` context. |
| **Borrow checker**            | `&World` (read-only) and `&mut World` (exclusive) are compile-time checked. The extract step borrows `&World` — no mutation possible. |
| **Lifetime annotations**      | Scoped worker-pool tasks cannot outlive the data they borrow. A `parallel_for` over component arrays is safe because the compiler proves the array outlives the task. |
| **No implicit copying**       | Large structs are never silently copied. If you want to share a `MeshAsset` across threads, you must explicitly wrap it in `Arc`. The cost is visible in the code. |

In a C++ engine, achieving the same guarantees would require either pervasive use of thread sanitizers (runtime detection, not prevention) or a team-wide discipline that scales poorly beyond a handful of engineers. Rust makes the guarantees structural.

## Putting It Together: The Frame in Motion

To see how the three pillars — the firewall, data-driven design, and thread ownership — interact in practice, let us trace a single frame through the engine.

### A Frame's Journey

```
time ──────────────────────────────────────────────────────────▶

Game Thread:
  ┌─────┐  ┌───────┐  ┌────────┐  ┌──────┐  ┌─────────┐  ┌───────┐
  │INPUT│  │ FIXED  │  │ UPDATE │  │ LATE │  │ EXTRACT │  │ CLEAR │
  │     │  │0–N×fix │  │  1×var │  │engine│  │diff+send│  │tracker│
  └─────┘  └───────┘  └────────┘  └──────┘  └────┬────┘  └───────┘
                                                   │
                                          owned FramePacket
                                                   │
                                                   ▼
Render Thread:
  ┌───────┐  ┌───────┐  ┌──────┐  ┌──────┐  ┌──────┐  ┌───────┐
  │RECEIVE│  │PREPARE│  │ CULL │  │RECORD│  │SUBMIT│  │PRESENT│
  │drain  │  │apply  │  │views+│  │graph │  │queue │  │swap + │
  │control│  │deltas │  │occl. │  │passes│  │submit│  │feedbk │
  └───────┘  └───────┘  └──────┘  └──────┘  └──────┘  └───────┘
```

1. **INPUT.** The main thread accumulates window events from the OS (via `winit`) into a polled `Input` snapshot. Events are never pushed into game logic — the OS callback pump is quarantined at this boundary.

2. **FIXED.** The engine runs `App::fixed_update()` zero to N times at a fixed timestep (default 60 Hz). This is where physics integration, network ticks, and deterministic gameplay logic execute. Zero ticks while paused; the accumulator advances by _scaled_ time, so `Time.scale` provides slow-motion with per-tick determinism intact.

3. **UPDATE.** `App::update()` runs once with variable delta time. Game systems mutate the `World` — spawning entities, moving characters, processing AI, reacting to events. Registered `System`s run in their declared `Phase` order.

4. **LATE.** Engine systems fire: hierarchy propagation (parent transforms flow down to children), bounds recomputation, streaming demand calculation. These are not user-visible — they are engine infrastructure preparing the world for extraction.

5. **EXTRACT.** The `ChangeTracker` is diffed against the World. Only dirty components generate delta entries. Fixed-tick transforms are interpolated at the accumulator's blend factor (smooth motion at any refresh rate). The result is a `FramePacket` — owned, `Send`, no references — pushed through the bounded channel to the Render Thread.

6. **CLEAR.** The change tracker resets. The Game Thread is free to begin the next frame.

On the Render Thread (one frame behind):

7. **RECEIVE.** Drain the control channel (resize, device events). Block on the packet channel until a `FramePacket` arrives.

8. **PREPARE.** Apply `ResourceOp`s: upload new meshes and textures from staging into GPU pools, update material uniforms, free resources for despawned entities. Apply scene deltas to the retained `RenderScene` — upserts and removes in the shared mesh draw store. This is the **only point** where the Render Thread mutates its persistent state.

9. **CULL.** Derive shadow views from lights. Collect TLAS instances (before per-view culling — rays need off-screen geometry). Run frustum culling and HZB occlusion culling per view, parallelized on the render worker pool. Select LODs with hysteresis. Sort and batch draws — opaque front-to-back, transparent back-to-front.

10. **RECORD.** The render graph executes: each pass records GPU commands — depth pre-pass, GBuffer, shadows, volumetrics, lighting, sky, transparency, post-processing.

11. **SUBMIT.** Throttle on GPU queue depth, then `queue.submit()`.

12. **PRESENT.** Blit to the swapchain. Send `FrameFeedback` (cull stats, GPU timestamps from the readback ring). Loop back to RECEIVE.

### What the Pillars Provide

Notice how each pillar contributes to the frame's smooth execution:

- **The Firewall** ensures that the Game Thread's UPDATE phase and the Render Thread's RECORD phase never touch each other's data. They run concurrently on separate frames without any synchronization beyond the channel send/receive.

- **Data-Driven Design** ensures that the EXTRACT phase is mechanical — iterate dirty sets, copy plain-data components into delta entries, move on. No virtual dispatch, no behavior callbacks, no side effects. The extract step is pure data transformation.

- **Thread Ownership** ensures that the PREPARE phase on the Render Thread is safe — the `RenderScene` is exclusively owned, and the deltas being applied are owned values received from the channel. No locks, no contention, no races.

Together, they produce an engine where the frame pipeline is deterministic, profilable, and scalable. Adding a new component type does not change the threading model. Adding a new render pass does not touch game code. And the compiler catches threading violations at build time, not in a crash report from a user's machine.

## Chapter Summary {.unnumbered}

This chapter established the three architectural pillars that underpin the Smallworld engine:

1. **The Game–Render Firewall** separates simulation from presentation through a hard boundary enforced by Rust's ownership system. Game code interacts with the renderer exclusively through owned-value `FramePacket`s sent via bounded channels. The Render Thread maintains a persistent `RenderScene` updated by deltas, not snapshots — making static entities free after their initial extraction. Feedback flows in the opposite direction through a separate channel, always advisory, never blocking.

2. **Data-Driven Design** replaces deep inheritance hierarchies with plain-data components stored in dense arrays, composed onto opaque entity IDs, and described by serializable asset descriptors. This design is cache-friendly, serialization-friendly, team-friendly, and — critically — extraction-friendly: components are inert data that can be safely read without side effects.

3. **Thread Ownership** eliminates shared mutable state from the engine's architecture. Each execution context — Game Thread, Render Thread, Audio Thread, Streaming Coordinator, split worker pools, IO pool — owns its data exclusively and communicates through CSP channels carrying owned values. Rust's type system enforces these boundaries at compile time, producing a lock-free engine where data races are not merely unlikely but structurally impossible.

In the next chapter, we will build on these foundations to examine the **core loop and frame lifecycle** in detail — the two-frame overlapped pipeline, frame pacing and vsync strategies, fixed-timestep simulation with interpolation, and the engine's lifecycle management for resizing, device loss, and graceful teardown.

## Review Questions {.unnumbered}

1. What is the narrow invariant that the Game–Render Firewall actually protects, and why is it narrower than "game code never touches the GPU"?

2. Explain the difference between delta-driven and snapshot-driven extraction. Why does delta-driven extraction make `EntityFlags::STATIC` meaningful?

3. Why does Smallworld use two separate worker pools (game and render) instead of a single shared pool? What specific failure mode does this prevent?

4. How does Rust's ownership system enforce the threading model at compile time? Give a concrete example of a threading bug that would compile in C++ but fail to compile in Rust.

5. The staging pool appears to violate the "no shared mutable state" principle. Why is it safe, and what three mechanisms ensure correctness?
