## Streaming

_(OQ 17 resolution, 2026-08-11.)_ Streaming is **two layers with different owners**, coordinated by demand signals and a single budget arbiter. Which _entities exist_ is Game Thread World mutation; which _data is resident_ is streaming-side truth (OQ 21). Conflating the two breaks the thread-ownership rules; splitting them is UE5's own shape (World Partition ∥ streaming pools).

### Layer 1 — World Streaming (entities)

The World Partition analog. Space divides into **uniform grid cells** — grids plural: independent grids per content class (e.g., 256 m gameplay entities, 1 km landmarks), each with its own cell size and load range; 2D partitioning by default, 3D as a config option. Uniformity is the point: O(1) cell lookup, stable cell identity (stable file names — what save references need), and predictable load sets (a source moving at speed _v_ crosses a computable number of cell boundaries per second, so worst-case IO is budgetable).

- **Entities auto-assign to cells by bounds**, with an `ALWAYS_LOADED` override for global entities.
- **`StreamingSource` drives loading.** Players/cameras carry one; cells within range load (entities batch-spawn — per the existing load-time-spawn rule), cells out of range unload (batch-despawn). Range rings with hysteresis prevent boundary thrash.

```rust
struct StreamingSource {
    range:    f32,   // load radius
    priority: u8,    // arbiter class for demand originating from this source
}
```

- **Cell content = OQ 20 documents: base + overlay.** The base is authored content or a generation cache; the overlay holds persistent runtime edits (destruction). Load = base ∪ overlay. Same serde document format as scenes and saves — one format for authored content, generated caches, and persistence. Cell files are named by grid coordinates: stable across sessions.
- **Unload persistence is a per-grid policy** _(OQ 35)_: `CellPersistence::Overlay` (default) diffs each unloading entity's dirty _registered_ components against the base document and writes the delta into the cell overlay — the pushed crate stays pushed when you return. `CellPersistence::Ephemeral` discards runtime state at unload — cells reset (respawning resources, dungeon instances). Per-grid, because the same world legitimately wants both (persistent structures grid + ephemeral clutter grid). Unregistered components are never persisted — the OQ 20 dev-mode audit warns.

#### Entity Lifecycle (spawn → live → despawn → memory)

_(OQ 35 resolution, 2026-08-12 — consolidation; every step already existed in its own section, this is the one-place ordering.)_

**Spawn** (cell load, descriptor, or game code — always batched at load time per the perf rule):

1. Registry-driven instantiation: stable component names → typed dense stores; `EntityRef` fields remapped; asset GUIDs resolve to handles (loads through the normal async path).
2. Side structures react off the `ChangeTracker` spawn set: physics bodies created from `RigidBody`/`Collider` descriptions; `AudioEmitter`s start (in range); `BehaviorRef` instances register in the `BehaviorHost` — `init` + first `update` run **next** frame.
3. The extract emits draw upserts + instance-slot allocations; the retained `RenderScene` absorbs them.

**Despawn** (cell unload, `despawn()`, or behavior command — always deferred to end of frame, per OQ 6):

1. Mark; children marked recursively (Entity Hierarchy).
2. If the cell policy is `Overlay`: dirty registered components diff into the cell overlay _before_ teardown.
3. End of frame, in order: behavior `shutdown`s → emitters stop (voices release to the pool) → physics bodies/joints destroyed → draw removes + instance-slot frees emitted through the delta → component entries removed (dense stores swap-remove) → **SlotMap slot recycled with a generation bump** — stale `EntityId`s and `PickId`s miss cleanly forever.
4. Asset handles held by the removed components drop; refcounts decrement; zero-ref assets become arbiter-evictable (never eagerly freed).

Memory story in one line: entity slots and component rows recycle immediately (arena + swap-remove); resource memory releases lazily under budget pressure. Allocation is eager and batched; deallocation is policy.

- **HLOD proxies:** the contract is reserved (a cell may carry a far-proxy entity set); generation tooling is deferred.

### Layer 2 — Detail Streaming (data residency)

Which data for existing entities is resident, at what quality. **v1 client: voxel bricks.** Texture mip streaming and mesh LOD streaming are later clients of the _same_ manager through the same thin client interface — designed for now, not retrofitted.

#### The Demand/Fulfill Pipeline

Every arrow is a channel; every stage uses machinery already specified:

```
Game LATE phase ── demand: (coord, wanted LOD, priority) ──▶ Streaming Coordinator
Coordinator ── dispatch ──▶ io_pool: region-file read  |  worker pool: VolumeSource::generate
tasks ── decode/write directly into ──▶ staging-pool regions (OQ 5)
Coordinator ── UploadBatch ──▶ Render Thread PREPARE: record GPU copies, publish residency
Render Thread ── FrameFeedback advisories (fulfilled / evicted / culled) ──▶ demand planner
```

- **The Streaming Coordinator is a dedicated low-priority thread** — it owns the priority queue and budget arbiter exclusively (thread-ownership applied, not excepted), and it is a _dispatcher, never a worker_: IO and decode run on the existing pools. Event-driven, parked when idle; completion-to-dispatch latency is microseconds, keeping the four-stage pipeline full instead of bubbling a frame per stage.
- **Cancellation** is generation-stamped queue entries — when the camera turns, stale demand dies in the queue, not in flight.
- **Residency publishes render-side** (brick pool tables) at copy-record time; the game thread only ever learns residency through advisory feedback, and never asserts it (OQ 21).

#### The Residency Invariant

**The coarse tier of everything is pinned resident** — SVO root/coarse bricks, lowest mips, far-tier extracted meshes. Every possible residency miss therefore has a rendering answer (fall back through coarser parents); the failure mode under any pressure is _blur, never holes, never stalls_. This is the virtual-texturing lesson applied engine-wide, and it is what makes every budget decision below safe to make.

Eviction above the pinned tier: **priority classes** (pinned → active-view → shadow/aux-view → prefetch), **LRU within class**, hysteresis (freshly-uploaded and recently-requested entries are evict-protected for a cooldown).

**LOD transitions gate on residency (OQ 10).** A fade-in never starts until the target LOD is resident; the demand rings _anticipate_ transitions by requesting the next LOD one band before its transition distance. Fade-_down_ is always possible unconditionally, courtesy of the pinned coarse tier. Transitions therefore never wait on IO and never pop because of it.

#### Budgets

Principle 5, cashed in: **GPU memory per pool, IO bandwidth, upload bytes per frame (= staging-ring capacity), and decode CPU time** are named budgets under one arbiter, allocated by priority class. Nothing streams "as fast as possible"; everything streams as fast as its budget.

#### Generation Caching

`VolumeSource` generation is deterministic, so caching is a per-source policy declared at registration — the cost profile belongs to the generator, not the engine (CPU PCG ranges from microseconds to seconds per brick, as prior experimentation showed):

```rust
enum GenerationPolicy {
    Always,       // regenerate on demand — cheap sources outrun disk IO
    CacheToDisk,  // generate once into region files — amortizes expensive generators
}
```

- Cache key: `(source name, source version, params hash, brick coord)` — version bumps invalidate precisely and automatically.
- **Contract rule:** `VolumeSource::generate` must be _pure_ with respect to `(params, coord)` — required for cache coherence, and it keeps the fixed-tick determinism story (OQ 19) open for generated worlds.
- **Edits are never cache.** Persistent modifications always live in the cell overlay, regardless of policy — the cache can be deleted wholesale at any time without losing player-visible state.

---

## Large-World Coordinates

_(OQ 30 resolution, 2026-08-11.)_ f32 breaks at planet scale — ~1 mm ULP at 10 km, ~1 cm at 100 km: vertex crawl, physics jitter, quantized gameplay truth. The answer is **f64 world space CPU-side with cell-anchored rendering** — UE5 LWC's direction, refined by our own streaming grid:

- **`Transform.position` is f64** (`DVec3`); rotation and scale stay f32 (their magnitudes never grow). `WorldTransform` translation is f64. View matrices are camera-relative (zero translation); the camera position rides `ViewParams` in f64.
- **Cell-anchored rendering preserves the retained scene.** Instance translations are stored **cell-local f32**, relative to their streaming cell's anchor — static, retained forever. Each frame, extract computes one f64 `(cell_anchor − camera)` offset **per cell**, not per instance; the vertex shader adds `pos_cell_local + cell_offset`, both bounded by streaming range and f32-safe. Naive per-instance camera-relative extraction was rejected — it would rewrite the entire retained instance buffer every frame, destroying the static-costs-zero property. The streaming grid provides anchors for free; integer cell coordinates are exact by construction.
- **Origin rebasing rejected:** a rebase rewrites every retained instance, cached bound, and physics body in one atomic event — a guaranteed hitch or smeared complexity — and it is hostile to future netcode (per-client origins vs. one canonical server frame). **Camera-relative-only with f32 world rejected:** it fixes rendering but leaves gameplay and physics truth quantized.
- **Physics runs the provider's double-precision mode** (rapier's f64 feature; Jolt's double-precision build) — f64 support joins determinism as a provider certification requirement (Physics — Provider Model, rule 3).
- **Cost honesty:** f64 halves SIMD lanes for transform propagation and culling math; mitigated by keeping only translations wide and using SoA layouts.
- **Always-on — no precision mode.** UE5 made LWC unconditional for the right reason: a precision _mode_ is a permanently under-tested code path. Flat-world games pay a negligible cost; the engine has one coordinate story.
