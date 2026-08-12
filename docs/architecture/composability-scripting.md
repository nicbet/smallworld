## Composability & Scripting

UE5's real power is the composability of its Actor-Component model and data-driven design, not the Blueprint visual editor. Smallworld takes the same composability principles and expresses them natively in Rust.

### The Entity-Component Model

Every entity in the world is an ID in a SlotMap. Behavior is assembled by attaching components — plain data structs. There is no `AActor` base class, no `UObject` hierarchy, no reflection macros. Components are registered by type and stored in dense, cache-friendly arrays.

This is the **Game Object Model** decision — Object-Oriented vs. ECS — and ECS is the answer (decided 2026-08-09, framing clarified 2026-08-11). It applies to the game-facing `World` only: engine internals (GPU pools, the retained `RenderScene`, streaming state) deliberately remain side structs and dense pools, where dense iteration wins and ECS query machinery adds nothing. Two questions, two answers.

```rust
// Composing an entity in code (equivalent to UE5's Actor constructor)
let player = world.spawn();
world.add(player, Transform { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE });
world.add(player, MeshRenderer { mesh: player_mesh, material: player_mat, cast_shadows: true, ..default() });
world.add(player, RigidBody { mass: 80.0, drag: 0.1, ..default() });
world.add(player, AudioListener);
```

This gives the same modularity as UE5's component attachment — snapping capabilities together to build complex entities — but it lives in plain Rust with full type safety and no runtime reflection overhead.

### Data-Driven Design

UE5 uses Data-Only Blueprints to avoid hardcoding asset paths in C++. Smallworld's equivalent is **asset descriptors** — serializable data files (RON, JSON, or a custom binary format) that describe entity archetypes.

```rust
// A game defines its entity archetypes as data, not code
struct EnemyDescriptor {
    mesh:     AssetPath,       // "meshes/goblin.glb"
    material: AssetPath,       // "materials/goblin.ron"
    health:   f32,
    speed:    f32,
    loot_table: AssetPath,
}
```

The engine loads these descriptors, resolves asset paths to handles, and spawns entities with the appropriate components. Artists and designers edit the data files; programmers define the descriptor schemas and the systems that process them.

### Behavior & Scripting

_(OQ 6 resolution, 2026-08-11.)_ Game behavior attaches to entities through **one contract with three backends**: Rust (native, first-class), C/C++ (native, via a stable C ABI), and Lua (sandboxed gameplay-iteration tier). A prototype scripted in Lua and later ported to Rust behaves identically — the tier changes performance, never semantics.

```rust
trait Behavior: Send {
    fn init(&mut self, entity: EntityId, ctx: &mut BehaviorContext);
    fn update(&mut self, entity: EntityId, ctx: &mut BehaviorContext, dt: f32);
    fn on_event(&mut self, entity: EntityId, ctx: &mut BehaviorContext, event: &dyn Event);
    fn shutdown(&mut self, entity: EntityId, ctx: &mut BehaviorContext);
}

struct BehaviorContext<'a> {
    world:    &'a mut World,
    input:    &'a Input,
    time:     &'a Time,
    events:   &'a mut EventBus,
    audio:    &'a mut AudioCommands,
    commands: &'a mut BehaviorCommands,  // deferred ops: the end-of-frame despawn queue
}
```

#### The BehaviorHost

Behavior instances live **outside** World component storage, in the `BehaviorHost` — a Game Thread side structure holding native Rust instances, C-ABI plugin instances, and the Lua VM. The entity carries only a plain-data `BehaviorRef` component (a behavior id). This dissolves the aliasing problem structurally — iterating the host mutably while borrowing the World is two disjoint borrows — and honors the plain-data component rule: behavior objects are not data.

#### The Three Backends

| Backend             | Linkage                                                             | World access                                    | Sandbox                  | Threading                                                                       |
| ------------------- | ------------------------------------------------------------------- | ----------------------------------------------- | ------------------------ | ------------------------------------------------------------------------------- |
| Rust                | Static; implements `Behavior` directly                              | Direct `&mut World` via context — zero overhead | No (trusted)             | Serial in v1; contract permits future parallelism via declared component access |
| C/C++               | Dynamic library; stable **C ABI vtable** mirroring `Behavior`       | Per-call accessor API (same surface as Lua)     | No (trusted native code) | Same as Rust                                                                    |
| Lua (mlua, Lua 5.4) | VM owned by the `BehaviorHost`; instances are registry-keyed tables | Per-call accessor API                           | Yes                      | Serial — a property of the single VM, not of the contract                       |

The **per-call accessor API** (`get`/`set` component, `spawn`, `despawn`, `emit`, audio commands, queries) is one surface serving both foreign backends — C cannot borrow-check any more than Lua can hold borrows across calls, so every call is a fresh, checked borrow. **Soundness rule (Lua):** userdata only ever wraps plain handles and copies (`EntityId`, component values) — never borrowed references into the World.

#### Uniform Mutation Semantics (all backends)

- **Spawn, add/remove component: immediate.** The returned `EntityId` is real and usable in the same call. (Borrow-sound because behaviors iterate from the host, not the World.)
- **Despawn: deferred to end of frame.** Entities are marked, then reaped after the frame's phases — the Unity `Destroy` / Godot `queue_free` convention that kills use-after-free ordering bugs. Generational IDs make any stale handle a clean miss regardless.
- **Behaviors spawned this frame start next frame** (`init` + first `update`).

#### Threading & Phase Placement

Behavior callbacks run in the variable-rate **Update** phase, sequentially on the Game Thread in v1. Serialization is scoped to where it is forced: the Lua backend is inherently serial (one VM; parallel Lua means multiple VMs with partitioned entities — deferred), while native backends may gain parallel execution later via declared component access, additively. Bulk per-frame logic belongs in **Systems** over component queries, not per-entity behaviors — that is the performance-first home and the first candidate for a parallel scheduler. One guard for the future: Lua's `pairs()` iteration order is nondeterministic, so admitting behaviors into `fixed_update` would require Lua-specific determinism rules first (the fixed-tick determinism guarantee currently covers engine systems).

### Gameplay Tags

Inspired by UE5's Gameplay Tags, smallworld uses hierarchical string tags for data-driven logic composition.

```rust
struct GameplayTag {
    path: InternedString,  // e.g. "status.debuff.burning"
}

struct GameplayTagContainer {
    tags: HashSet<GameplayTag>,
}

impl GameplayTagContainer {
    fn has(&self, tag: &GameplayTag) -> bool;
    fn has_any(&self, tags: &[GameplayTag]) -> bool;
    fn has_all(&self, tags: &[GameplayTag]) -> bool;
    fn matches_prefix(&self, prefix: &str) -> bool;  // "status.debuff.*"
    fn add(&mut self, tag: GameplayTag);
    fn remove(&mut self, tag: GameplayTag);
}
```

Tags enable systems to interact without hard coupling. A fire ability applies a `status.debuff.burning` tag; the damage system queries for `status.debuff.*` tags and applies tick damage; the VFX system queries for the same tags to spawn particle effects. None of these systems know about each other — they communicate through data.
