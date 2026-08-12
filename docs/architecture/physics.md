## Physics

_(OQ 16 resolution, 2026-08-11.)_

### Provider Model

Physics is a **provider behind one engine-shaped interface** — the same replaceable-node philosophy as the temporal resolve and the behavior backends. v1 provider: **rapier** (pure Rust, zero FFI, deterministic mode). Named up/side-grade candidates: **Jolt** (quality and scale headroom; C++ FFI) and PhysX. Swapping providers is a port of one module, never a rip-and-replace — because the interface obeys three rules that keep the swap real:

1. **Shaped by engine consumption, never by provider wrapping.** Descriptions in (`RigidBody`/`Collider` components), transforms + events + query answers out. The interface covers what the engine _uses_, not the union of provider features.
2. **No lowest-common-denominator bloat.** Provider-specific capability goes through a typed extension escape hatch (`provider.extension::<JoltExt>()`) — games use it knowingly, at their own portability cost; the core interface never grows to accommodate one provider.
3. **Determinism and precision are part of the contract.** A provider must offer a deterministic mode to be certified for fixed-tick use (the OQ 19 guarantee), and a double-precision mode for large-world coordinates (OQ 30).

```rust
trait PhysicsProvider: Send {
    // Lifecycle from component descriptions (change-tracker driven)
    fn create_body(&mut self, entity: EntityId, body: &RigidBody, collider: &Collider,
                   transform: &Transform) -> PhysicsHandle;
    fn update_body(&mut self, handle: PhysicsHandle, body: &RigidBody);
    fn destroy_body(&mut self, handle: PhysicsHandle);

    // Simulation
    fn step(&mut self, fixed_dt: f32);

    // Sync-back: fixed-tick transforms + events out
    fn drain_transforms(&mut self, out: &mut Vec<(EntityId, Transform)>);
    fn drain_events(&mut self, out: &mut Vec<PhysicsEvent>);  // contacts, triggers

    // Joints (OQ 28)
    fn create_joint(&mut self, a: PhysicsHandle, b: PhysicsHandle, joint: &Joint) -> JointHandle;
    fn destroy_joint(&mut self, handle: JointHandle);

    // Queries — the game-thread read API
    fn raycast(&self, ray: Ray, filter: QueryFilter) -> Option<RayHit>;
    fn sweep(&self, shape: &Shape, motion: &Motion, filter: QueryFilter) -> Option<SweepHit>;
    fn overlap(&self, shape: &Shape, filter: QueryFilter) -> Vec<EntityId>;
}
```

### Integration

- **Physics steps exclusively in `fixed_update`** — determinism and solver stability both demand it. This closes the other half of OQ 6's interpolation contract: physics writes fixed-tick transforms; extract interpolates them.
- **The physics world is a side structure.** `RigidBody` and `Collider` are plain-data _descriptions_; the provider owns simulation state internally, linked by handle, created/destroyed from the change tracker's spawn/dirty sets. Components stay serializable (OQ 20) — simulation state rebuilds from descriptions on load.
- **Sync-back** after each fixed step goes through the normal `get_mut` path (change tracking fires naturally), storing prev-tick state for interpolation. `PhysicsEvent`s enter the double-buffered event bus.
- **Queries** are the game-thread read API — OQ 22's gameplay raycasts ride this for collider-bearing entities.

### Joints & Constraints

_(OQ 28 resolution, 2026-08-11.)_ Typed joints — the Unity (typed + `ConfigurableJoint`) / UE (typed profiles over a generic core) shape: readable descriptions for the 95%, `SixDof` as the fully-general escape member. Plain data; the paired body is an `EntityRef`, so joints survive save/load remapping (OQ 20) like every other reference. Breaking emits an event into the bus and removes the joint; motors are v1.

```rust
struct Joint {
    other:     EntityRef,      // the second body — serialization-remappable
    kind:      JointKind,
    anchors:   (Vec3, Vec3),   // local-space anchor per body
    breakable: Option<f32>,    // force threshold → break event + removal
}

enum JointKind {
    Fixed,
    Revolute  { axis: Vec3, limits: Option<(f32, f32)>, motor: Option<JointMotor> },
    Prismatic { axis: Vec3, limits: Option<(f32, f32)>, motor: Option<JointMotor> },
    Spherical { cone_limit: Option<f32> },
    Distance  { min: f32, max: f32 },
    SixDof    (SixDofConfig),  // per-axis limits/motors — the escape hatch
}
```

### Character Controller

_(OQ 28 resolution, 2026-08-11.)_ An **engine-owned kinematic character controller**, built exclusively on the provider _query_ API — shape casts and overlaps, portable primitives every provider implements. Game feel is therefore **provider-invariant by construction**: a rapier→Jolt swap changes dynamic-body solver behavior, never how the player moves. Provider-native controllers (rapier KCC, Jolt `CharacterVirtual`) were rejected for exactly that feel-drift; dynamic-body characters for feel, period. This is the Unity `CharacterController` / UE `CharacterMovementComponent` shape — both engines own their controller rather than delegating to the physics library.

```rust
struct CharacterController {
    capsule:     Capsule,  // radius, height
    step_height: f32,      // max auto-step ledge
    slope_limit: f32,      // degrees — steeper surfaces are walls
    ground_snap: f32,      // snap-down distance (stairs, small dips)
}
```

- **Move-and-slide in fixed tick:** iterative shape-casts, slide along surfaces, step-up, slope rejection, grounding state, moving-platform inheritance.
- **Kinematic-vs-dynamic:** dynamic bodies never push the controller; the controller applies impulses to dynamic bodies it contacts (crates push, players don't get shoved by ragdolls).
- **Seams already built converge here:** root motion (OQ 25) routes displacement into `move()`; grounding state feeds `Animator` parameters; fixed tick + interpolation (OQ 6) keeps it smooth at any refresh rate.

**Ragdolls (future, pure composition):** joints + a pose-blend layer on the `Animator` — both halves now exist; no new contract needed. **Vehicles (future, module):** deliberately deferred with zero design debt — they compose entirely from primitives that now exist (wheel shape-casts, suspension impulses, joints, fixed tick); there is no single vehicle model to standardize (arcade raycast through full drivetrain sim spans genres); and the references agree it's module territory — UE's Chaos Vehicles is literally a plugin. First model when needed: the raycast-vehicle workhorse, provider-portable like the KCC.

### Worker-Pool Split

**v1: two pools** — a render pool and a game pool, sized by core count (configurable). Culling never waits on physics: with no preemption in any Rust task system, isolation is the only _guaranteed_ fix for priority inversion. The classic utilization objection evaporates under our own architecture — the 2-frame pipeline runs game frame N+1 and render frame N concurrently, so both pools stay busy in steady state. The physics provider's internal parallelism binds to the game pool.

**v2: a task-graph scheduler** — declared dependencies + priorities (the UE Task Graph analog), built on crossbeam's work-stealing primitives (`crossbeam-deque`, the same foundation rayon stands on). One future scheduler serves parallel systems (OQ 6), physics, and streaming decode alike.
