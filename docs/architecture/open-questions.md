## Open Questions

Round 1 (OQ 1–23): the decision backlog from the 2026-08-11 review round — **fully resolved as
of 2026-08-11**. Entries are kept as decision records: each captures the choice, the rationale,
the rejected alternatives, and where the spec lives. Deferred items _inside_ resolutions (v2
tiers, capability-gated upgrades, scheduled hardening) carry their own explicit adoption
triggers and need no separate tracking here.

Round 2 (OQ 24–28): opened 2026-08-11 from the Gregory (_Game Engine Architecture_) subsystem
audit — the chapters the doc only covered where they intersected the render pipeline. Framing
rule for all five: **decide the engine primitives and traits; games compose them** — Gregory
intermixes game and engine concerns, we deliberately do not.

Round 3 (OQ 29–30): opened 2026-08-11 during the Voxel Plugin design kickoff — vegetation
surfaced as a missing subsystem, and large-world coordinates escalated from the plugin's V2
(see `voxel-plugin-design.md` for the plugin's own V-series).

1. **[RESOLVED 2026-08-11] Volume depth writes & motion vectors.** Fragment-shader raymarch over
   rasterized per-object proxy AABBs with `frag_depth` export — one shader writes depth, the full
   GBuffer, and velocity; its depth-only variant implements `ShadowCaster`. Full spec: "Volume
   Rendering Mechanism" in the GBuffer stage. The compute visibility-buffer variant (64-bit
   atomic min/max, `Capabilities::int64_atomic_min_max`) is the sanctioned capability-gated
   upgrade tier, scheduled with the GPU-driven work (OQ 7); adoption trigger: profiling shows the
   fragment path limiting on capable hardware, or variable-rate marching is needed.
2. **[RESOLVED 2026-08-11] RT-input binding mechanism.** Always-bound neutral dummies +
   per-frame uniform flag bits — one pipeline per consumer, uniform branching (coherent, ~free),
   runtime toggles without pipeline rebuilds. Adopted as the render graph's **general
   optional-input-slot mechanism**: the implementation of every public input slot (GI, shadow
   masks, sky visibility, future slots). Pipeline permutations remain a targeted optimization
   behind a profiling trigger. Spec: Render Graph — Optional Input Slots.
3. **[RESOLVED 2026-08-11] Hit-point radiance (was: surface cache).** Hybrid by ray character,
   staged: **v1 — all RT hits sample the GI clipmap** (the lit clipmap is the engine's
   world-radiance representation, maintained whenever software GI _or_ RT is active; Lumen's
   surface-cache role at voxel fidelity, zero marginal machinery). **Later — bindless
   hit-shading for sharp near reflections** (binding arrays, ray-cone LOD; capability-gated),
   with the world-space-light-structure caveat noted for off-screen hits. The v2/v3 surface
   cache inherits the role for both paths. Spec: RT GI / RT Reflections / Software GI Tier.
4. **[RESOLVED 2026-08-11] Core software-GI tier.** Technique: **screen traces + cascaded
   voxel cone tracing over an engine-owned lighting-domain GI clipmap** (SVOGI family; KCD 1/2
   precedent). Geometry via conservative raster of the shared mesh stream; plugins inject
   directly through the **GI injection point** (Voxel Plugin: SVO data, destruction-fresh).
   Same structure yields cone-traced sky visibility (upgrading the OQ 11 baseline for all
   games) and rough-specular cones (new middle rung in the reflection chain). Known costs
   named: VCT thin-wall leaking, ~100–200 MB clipmap budget, sub-Lumen quality. **Roadmap
   commitment: the smallworld Lumen analog (mesh SDFs + surface cache) is the v2/v3 quality
   end-state** — a representation swap behind unchanged public slots; the clipmap survives as
   far-field fallback. SDFGI rejected (static-biased generation, needs a voxel radiance cache
   anyway); DDGI probes = encoding option, not a representation; SSGI = first hop, kept. Spec:
   Software GI Tier (Lighting Pass).
5. **[RESOLVED 2026-08-11] Asset payload transport.** Option B — the engine-owned **staging
   pool**: decode threads write directly into mapped staging regions (no payload memcpy on any
   hot thread); `ResourceOp` carries `StagingRef` handles; the Render Thread records GPU copies
   only — O(1) per upload. Principles clarified alongside: Principle 2 permits shared
   _immutable_ data; Principle 3 constrains game code, and the real invariant is Render-Thread
   ownership of device-local resources + submission — engine subsystems may create/populate
   staging off-thread. Small payloads stay by-value; `Arc` transport remains legal internally.
   The pool is shared with OQ 17 streaming and joins the OQ 15 teardown protocol. Spec: Staging
   Pool & Upload Path (Data Structures).
6. **[RESOLVED 2026-08-11] Gameplay-layer semantics.** (1) **Events: double-buffered** — sent
   frame N, readable frame N+1, deterministic under any system order. (2) **Behavior model:**
   one `Behavior` contract, three backends — Rust (native, direct `&mut World`), C/C++ (stable
   C ABI vtable), Lua via mlua (sandboxed) — instances living in the `BehaviorHost` outside
   World storage (aliasing dissolved structurally); both foreign backends share one per-call
   accessor API; uniform semantics everywhere: spawn/add/remove immediate, despawn deferred to
   frame end; Lua's serialization is a backend property, not a contract property. (3)
   **Fixed-step interpolation at extract** — lerp of the last two fixed-tick states at
   accumulator alpha; camera passes through; teleports snap; distinct from the motion-vector
   prev-frame matrix. Specs: Behavior & Scripting, Fixed-Timestep Interpolation, Events.
7. **[RESOLVED 2026-08-11] GPU-driven rendering.** v1 is CPU-driven by design; GPU-driven
   (GPU scene buffer → GPU frustum+HZB culling → indirect draws → visibility-buffer geometry)
   is the **sanctioned phase-2 direction**. **Hard requirement: phase 2 must be additive.** The
   prepared contracts (instancing-capable `MeshDrawCommand`, dense pools, retained scene,
   per-view culling, 64-bit-atomic capability flags) are shaped so it slots in; if any phase-2
   design turns out to demand contract rework, that rework returns to discussion before
   implementation. Adoption trigger: profiling shows CPU culling or draw submission limiting at
   target scene scales.
8. **[RESOLVED 2026-08-11] Frame pacing & latency control.** Three control loops on existing
   machinery: (1) **GPU queue-depth throttle** (v1 — bounded deterministic latency, default 1
   frame in flight); (2) **DRS controller** (v1 — GPU time vs. target drives
   `resolution_scale`; asymmetric, hysteresis-banded, step-limited); (3) **predictive tick
   pacing** (v2 — Reflex-style, `LatencyMode::LowLatency`, margins game-tunable, calibrated
   against real content). `PacingConfig` in `EngineConfig`; Lockstep remains the blunt
   instrument. Spec: Frame Pacing & Latency Control (Frame Pipeline).
9. **[RESOLVED 2026-08-11] Translucency lighting & volumetrics.** Three-part resolution:
   (1) transparent surfaces shade via Clustered Forward+ reusing the deferred light grid, shadow
   atlas, and shading models, with refraction from `scene_color_copy` and no OIT in v1;
   (2) participating media lives in a core froxel volumetric system with a **public injector
   contract** (`FogVolume` component + `EnvironmentParams` height fog); (3) translucent voxel
   media is plugin-side, two tiers — froxel injection far, raymarched media pass in the
   transparency stage near; the opaque `VolumePass` never renders media. Specs: Volumetrics and
   Transparency stages. Dependencies flagged into OQ 11 (env/fog params, translucent specular)
   and OQ 12 (upscaling vs. froxel/half-res media resolution).
10. **[RESOLVED 2026-08-11] Seamless LOD transitions.** Core/plugin split confirmed: **core
    provides mechanism, backends own policy.** Core: per-instance `fade` + complement bit
    honored by all shared-stream passes via a **public screen-door dither convention**, TAA as
    resolver, selection hysteresis, and residency gating with demand anticipation (Streaming).
    Mesh LODs: dithered cross-fade (hard switch and geomorphing both tested and rejected).
    Voxel Plugin (private designs on public contracts): distance-banded dual-LOD blending in
    the raymarched tier (clipmap-style, stateless) with late-arrival fade-in; handoff =
    extract-from-same-LOD convergence + complementary dither band. Zero voxel-specific engine
    hooks needed. Specs: LOD Transitions (Mesh Drawing Pipeline), Volume Rendering Mechanism,
    Streaming.
11. **[RESOLVED 2026-08-11] IBL & reflection probes.** Resolution "A′": environment pipeline
    (sky capture → GGX-prefiltered specular mips + SH9 irradiance + split-sum LUT, amortized
    updates) with **mandatory sky-visibility modulation** of the environment term — baseline
    bent-normal/SSAO specular occlusion in core; upgraded via the public `sky_visibility` input
    slot, fed by the Voxel Plugin's SVO-traced directional visibility (destruction-proof). SSR
    is always-on; specular chain: RT → SSR → probes → sky × visibility. GI upgrade path runs
    through the same public slots (OQ 4). `EnvironmentParams` defined (including the OQ 9
    height-fog rider); transparent env term paid; `ReflectionProbe` spec'd but deferred —
    trigger: _authored_ interiors. Specs: Environment Capture & IBL (Sky stage), Lighting Pass
    chains, Core Engine Components.
12. **[RESOLVED 2026-08-11] Post chain completeness.** Internal vs. display resolution split
    made first-class (scene targets at internal res; DRS reserved via
    `ViewParams.resolution_scale`, control loop owed to OQ 8). **TAAU in-house for v1** — TAA
    and upscaling as one temporal resolve, native = scale 1.0, spec'd as a replaceable graph
    node; **FSR 2.2 (WGSL) in v2** through that slot; **DLSS when practical in wgpu**
    (`dlss_wgpu`-style interop; NVIDIA + Vulkan). **Auto-exposure**: histogram with percentile
    clamps + asymmetric adaptation, `Manual(ev)` mode, advisory via feedback. Tone mapping
    defaults to **ACES**. Froxel and hero-media buffers count as internal-res scene targets
    (the OQ 9 rider resolves with the split). Spec: Post-Processing stage.
13. **[RESOLVED 2026-08-11] Decals.** Core engine feature, shipped as standard **deferred
    GBuffer decals** — projected boxes after opaque geometry, before lighting, blending into
    existing targets. Contract touches done now: `DrawFlags::RECEIVE_DECALS` reserved;
    octahedral normal decode/blend/re-encode noted. Purely additive pass, so implementation is
    scheduled after the v1 rendering core with zero design debt. Spec: Deferred Decals (GBuffer
    stage).
14. **[RESOLVED 2026-08-11] Skinning.** Option B, generalized into a **Deformation stage**
    (`DeformPass`, render pipeline stage 3): compute pre-skin into per-instance buffers — skin
    once, consumed identically by depth, GBuffer, shadows, and BLAS refit — with skinning as
    the built-in deformer and morphs/cloth/procedural as registered deformers. Velocity via the
    `position_prev` buffer-aliasing rule (no shader permutations anywhere). Named memory
    budget; deliberately no vertex-shader fallback in v1. Animation sampling stays CPU-side;
    palettes ride the staging pool. Spec: Deformation stage.
15. **[RESOLVED 2026-08-11] Resize / device-lost / teardown.** Full protocol in the new
    **Lifecycle** section. (1) **OOB control channel** (Gregory-style control/data separation)
    with **frame-boundary application** — transport independent of packet flow, application
    synchronized so frame content and surface config stay atomic. (2) Device loss: the
    **GPU-memory-is-a-cache invariant** made law (recovery permanently possible); v1 =
    fatal-with-grace via save hook; recovery walk = scheduled hardening. (3) **Staged teardown
    protocol** (simulate-stop → producer-quiesce with region-file flush → pipeline drain +
    GPU idle → services → destroy), channel-closure backstop, per-stage deadlines, never
    hangs. Staging pool and readback ring drain in stage 3 as required by OQ 5.
16. **[RESOLVED 2026-08-11] Physics architecture.** **Provider model**: one engine-shaped
    `PhysicsProvider` interface (shaped by engine consumption, typed extension escape hatch,
    determinism required for certification) — **rapier v1**, Jolt/PhysX as named up/side-grade
    candidates, swap = one-module port. Integration: fixed-tick-only stepping (closes OQ 6's
    interpolation contract), physics world as side structure with plain-data
    `RigidBody`/`Collider` descriptions (OQ 20-serializable), sync-back via `get_mut`, events
    into the double-buffered bus, queries as the game-thread read API (OQ 22 rides it).
    **Worker pools: split game/render pools in v1** (isolation beats non-existent preemption;
    the 2-frame pipeline keeps both busy); **crossbeam-based task-graph scheduler in v2**,
    unified with parallel systems and streaming decode. Spec: Physics section.
17. **[RESOLVED 2026-08-11] Streaming.** Full two-layer design in the new **Streaming**
    section. Six locked decisions: (1) two layers — World Streaming (entities, game side) ∥
    Detail Streaming (data residency, streaming side); (2) uniform grid cells, grids plural,
    2D default; (3) dedicated low-priority Streaming Coordinator thread — dispatcher, never a
    worker, on the existing pools; (4) pinned coarse tier + class-LRU eviction with hysteresis
    (blur, never holes); (5) cell content = OQ 20 documents, base + overlay; (6) per-source
    `GenerationPolicy::Always | CacheToDisk` with pure-generation contract and cache keyed by
    (name, version, params, coord). Upload backbone = staging pool (OQ 5); region files back
    saves (OQ 20); residency truth with the brick pool, demand-only game side (OQ 21).
18. **[RESOLVED 2026-08-11, amended same day] UI.** Two-track: **dev/debug tooling = egui**
    (final render-graph pass, dev builds). **Game-facing UI: architectural stance committed** —
    **widgets are entities** (Godot's unification translated to ECS, Bevy-proven; a Slate-style
    separate tree rejected as the reason UMG had to exist). Menus are OQ 20 documents;
    behaviors attach to widgets; events ride the bus; themes are assets; flexbox-class layout +
    anchors (`taffy` precedent); display-res UI pass after post, never DRS-scaled; UI-first
    input routing with focus semantics. Widget catalog / theming / text / l10n / a11y deferred
    to the UI design round. **Editor = an application on the engine** (Godot's proof) — not
    engine core; designed only once there is a runtime to edit. Spec: UI Framework section.
19. **[RESOLVED 2026-08-11] Networking.** Explicitly **out of scope for v1**, with the hooks
    accounted for now: `fixed_update` is the deterministic tick (with the engine guarantee that
    no engine system introduces nondeterminism into fixed-tick simulation — see The App Trait),
    components are plain replicable data, entity IDs are generational. Networking arrives
    post-v1 as transport + replication modules on those hooks; replication consumes the OQ 20
    component registry (stable identity + codecs) rather than growing its own.
20. **[RESOLVED 2026-08-11] Save / serialization.** Option C — opt-in component registry
    (stable name + version, serde codecs, migration hooks) + save documents + fresh-ID loading
    with `EntityRef` auto-remapping; transient state rebuilt, never saved ("persistent state
    lives in components" as a load-bearing rule); bulk world data fenced off to streaming's
    region files. The registry is shared with replication (OQ 19) and future editor reflection.
    Implementation scheduled with the first save-game need. Spec: Serialization & Save Games
    (World Building).
21. **[RESOLVED 2026-08-11] Voxel Plugin data ownership.** Both halves resolved B: (1)
    `VolumeRenderer.source` is a **`VolumeSourceId` handle** into a plugin-side generator
    registry (stable names → serializable like asset paths; per-entity params as plain data) —
    the plain-data component rule holds everywhere again. (2) **Residency truth lives with the
    brick pool** (streaming side); the game thread expresses demand only; `brick_residency`
    deleted from `VolumeDrawCommand`; `VolumePass` falls back through coarser SVO parents — the
    virtual-texturing pattern. Demand/fulfill protocol design merged into OQ 17.
22. **[RESOLVED 2026-08-11] GBuffer ID reconciliation.** The spec **cuts the per-frame ID
    target entirely** (GBuffer is six targets; ~4 B/px bandwidth saved). Gameplay picking = CPU
    BVH raycast; tools/editor = on-demand scissored pick pass writing tagged `PickId`s, returned
    via the readback ring with generation validation; debug heatmaps = the same pass
    full-screen, on demand. The mesh/volume source flag lives in the albedo-alpha flag bits,
    debug-tooling-only — no lighting or post pass may branch on it. Implementation (sw-6dd982's
    persistent material-ID target) migrates to match the spec. Spec: Picking & Debug IDs
    (GBuffer stage).
23. **[RESOLVED 2026-08-11] CLAUDE.md entity-model reconciliation.** The old CLAUDE.md text
    conflated two questions. Clarified framing: the sw-cf6350 benchmarks evaluated ECS as a
    general _engine-internals_ mechanism (answer: no — side structs and dense pools win there);
    the **Game Object Model** decision was always Object-Oriented vs. ECS, and ECS is the modern
    consensus (decided 2026-08-09). CLAUDE.md rewritten with the two-question split; this doc's
    Entity-Component Model section states it too. One source of truth restored.
24. **[RESOLVED 2026-08-11] Profiling & instrumentation.** One instrumentation API, multiple
    sinks (the UE stat+Insights / Unity ProfilerMarker shape): scope/counter/gauge macros with
    named lanes for all execution contexts, zero-cost in shipping builds; sinks = **Tracy**
    (deep timeline, lock/memory profiling, GPU lanes fed by the readback ring), **egui
    overlay** (frame graph, top-N scopes, counters, generated budget table), **chrome-trace
    export** (CI, bug reports). **BudgetRegistry**: every named budget registers a gauge at
    creation — receipts are structurally mandatory; DRS and the streaming arbiter read the
    gauges they publish. Standard counter set spec'd; `OcclusionFeedback` remains the
    game-facing advisory path. Shipping telemetry out of scope v1 (trace export is its hook).
    In-engine-only rejected (rebuilds Insights); Tracy-only rejected (no always-on budget
    receipts). Spec: Profiling & Instrumentation section.
25. **[RESOLVED 2026-08-11] Animation runtime.** Option D — **layered: pose primitives as
    public engine core** (sample/blend/mask/additive, two-bone + look-at IK nodes), **with the
    data-driven `AnimGraph` evaluator built on them** (blend trees, layers, state machines as
    serde/RON assets; games drive parameters from behaviors). UE/Unity are both secretly this
    shape — we design the layering they retrofitted. Riders: `SkeletonAsset`/`AnimClipAsset`
    from glTF with cook-time ACL-style compression (OQ 27); `Animator` plain-data component,
    sampling on the game pool in PostUpdate, palette → staging → DeformPass; events into the
    double-buffered bus; root motion delivered never auto-applied; bone sockets via
    `attach_to_bone`. Spec: Animation section.
26. **[RESOLVED 2026-08-11] Audio engine.** **In-house** (Option B): `cpal` device layer;
    audio thread mixing at fixed block size; data-defined mixer/bus graph (`MixerLayout`
    asset); voice pool with priorities + virtualization; distance/pan spatialization (HRTF
    deferred); per-voice DSP knobs + one send-bus reverb + an effect-insert trait; streaming
    music via IO-pool ring buffers; occlusion = engine knob, game computation via physics
    queries. Middleware deliberately unspec'd — a whole-subsystem replacement plugin remains a
    possible future path if a shipping need demands it. v2+: MetaSounds-style procedural
    graphs. Spec: Audio section. _Amended 2026-08-12:_ **`AudioEmitter`** declarative component
    added (engine-managed lifecycle for persistent entity-attached sources — the
    imperative/declarative pairing), and the **engine-never-initiates doctrine** recorded with
    the trigger/footstep compositions ("Who Initiates Sound").
27. **[RESOLVED 2026-08-11] Resource pipeline & filesystem.** **Identity: GUIDs** assigned at
    import (sidecar `.meta`, Unity-proven), paths as human-facing aliases — resolves OQ 20's
    ambiguity. **Cooking:** asset database + derived-data cache keyed by
    `(source hash, importer version)` — cook-on-demand in dev (hot reload rides it), full cook
    via `smallworld-cook` for shipping; shipping runtime reads cooked formats only, no
    importers; `AssetLoader` relocated to cook time. **VFS:** `content://` (loose dev / zstd
    pak shipping), `user://` (saves + overlays), `temp://`; mods structurally reserved as
    higher-priority mounts. **Dependencies:** recorded at import, loaded as closures; zero
    refs = evictable-not-evicted, eviction by budget arbiter (GPU-is-a-cache extended to CPU).
    Spec: Resource Pipeline & Filesystem section.
28. **[RESOLVED 2026-08-11] Physics contract completion.** **Joints: typed enum** (Fixed /
    Revolute / Prismatic / Spherical / Distance + `SixDof` escape hatch — the Unity/UE shape),
    plain data with `EntityRef` (OQ 20-remappable), breakable → event, motors in v1; provider
    API gains `create_joint`/`destroy_joint`. **Character: engine-owned kinematic controller**
    on the portable query API only — game feel provider-invariant by construction
    (provider-native controllers rejected for feel-drift on swap; dynamic-body characters for
    feel). Move-and-slide, step-up, slopes, ground snap, platform inheritance; impulses out,
    no push-in. Ragdolls = future pure composition (joints + Animator blend). Vehicles =
    future module, deferred with zero design debt (compose from existing primitives; no single
    model to standardize; Chaos Vehicles is a plugin too). Spec: Physics section.
29. **[RESOLVED 2026-08-11] Vegetation, foliage & the PCG framework.** Graph-based PCG running
    at **cell-generation time in the streaming pipeline** — deterministic (seeded by cell +
    graph hash), with **deterministic thinning keys** making density/distance thresholds
    re-scatter-free. **Two output tiers per prototype**: entities (heavy — cell documents,
    identity, colliders) vs. pure instance sets (light — no entities; overlay removal-marks by
    stable key). Rendering: per-cell per-prototype batches on the shared mesh stream
    (cell-granular culling, v1-viable), cook-time imposters as the far tier, **foliage named
    first customer of OQ 7 phase 2**. Wind = material vertex animation from
    `EnvironmentParams::wind`, deliberately not the DeformPass. Persistence per OQ 17/20.
    `ScatterSurface` trait engine-owned; the Voxel Plugin implements it via V6. Spec:
    Vegetation & PCG section.
30. **[RESOLVED 2026-08-11] Large-world coordinates.** **B′ — f64 world space CPU-side +
    cell-anchored rendering**: `Transform.position` is `DVec3`; instance translations stored
    cell-local f32 (static, retained); one f64 `(anchor − camera)` offset per cell per frame —
    the retained scene's static-costs-zero property survives, which naive camera-relative
    extraction would have destroyed. Origin rebasing rejected (rewrites the retained world
    atomically; netcode-hostile); camera-relative-only rejected (gameplay truth stays
    quantized). Physics providers must offer double-precision (certification rule 3 amended).
    Always-on — no precision mode (the UE5 LWC lesson). Spec: Large-World Coordinates
    section.
31. **[RESOLVED 2026-08-11] Atmosphere, clouds & weather.** Fog was already complete (OQ 9);
    this adds the sky half. **Atmosphere:** engine-provided Hillaire LUT model (the UE5
    SkyAtmosphere / HDRP shape) — planetary, correct from ground to space (V2 worlds), feeding
    IBL and the froxel far field automatically. **Clouds:** engine module — Schneider-class
    volumetric layer in its own altitude-slab pass (not froxels), cloud shadows via the
    shadow-mask slot; scheduled post-v1-core, skybox interim. **Weather:** engine owns
    `WeatherState` + plumbing (cloud/fog/wind coupling, wetness/snow material hooks in
    `Standard`); games own the logic; precipitation = instanced layer in the module, not
    blocked on OQ 32. Spec: Atmosphere, Clouds & Weather section.
32. **Particles & particle system.** The engine has no general particle/VFX subsystem —
    `ParticleBackend` appears in this document only as the canonical _example_ of a custom
    geometry backend. A dedicated design round (post-v1, like the game-UI round) owes:
    emitter/simulation model (CPU and GPU tiers), the authoring shape (the data-graph pattern —
    a Niagara / VFX-Graph analog), rendering integration (custom lane vs. shared-stream
    sprites/meshes; transparency ordering; froxel-lit particles), collision (depth-buffer +
    physics queries), a determinism stance, and the engine/game split (engine: simulation +
    rendering primitives + graph evaluator; games: effect graphs as assets).
33. **[RESOLVED 2026-08-12] Game-flow primitives (worlds, pause, runtime settings).** Surfaced
    by the scene-change walkthrough (menu → loading → game → pause). The flow state machine is
    game code; the engine provides: **background world construction + frame-boundary swap**
    (scene-reset delta clears the retained scene; assets survive via refcounts; physics
    rebuilds from descriptions; commands-audio survives, emitters die — lifetime encoded by
    the imperative/declarative split); **pause & time scale** (`Time.scale/paused/real_dt`;
    paused freezes the fixed accumulator while `update` continues on `real_dt`); **runtime
    settings** (`set_pacing`/`set_window_mode`; `EngineConfig` = initial values only). Specs:
    Worlds & Game Flow (World Building), GameContext, Time, The App Trait. _Amended same day:_
    menus/loading screens are **live Worlds** (pause is the in-game tool; paused/scale persist
    across swaps, never implicitly changed); dual shader clocks (scaled + real elapsed in frame
    uniforms); **`SwapTransition::CaptureLastFrame`** freeze-frame crossfade (dual-world live
    crossfade explicitly out of scope).
34. **[RESOLVED 2026-08-12] Input action mapping.** Named, rebindable, device-agnostic actions
    over the raw polling API: `ActionMap` assets (Button/Axis1/Axis2, composite WASD, dead
    zones), a **context stack** with block/passthrough — the input half of OQ 18's
    focus/capture — rebinds persisted to `user://`, and the documented frame-edge → intent →
    fixed-tick consumption pattern. Engine owns machinery, stack, persistence; games declare
    maps as data. Spec: Input — Action Mapping.
35. **[RESOLVED 2026-08-12] Entity lifecycle & cell-unload persistence.** Surfaced by the
    Gregory world-loading audit: spawn/despawn existed only scattered across six sections, and
    **entity-state persistence at cell unload was genuinely unspecified** (voxel edits
    persisted via V8; a player-pushed crate did not). Resolution: (1) a consolidated **Entity
    Lifecycle** ordering — registry instantiation → side-structure reactions off the change
    tracker → render deltas; deferred despawn → overlay diff → ordered teardown → generational
    slot recycling → lazy asset eviction ("allocation is eager and batched; deallocation is
    policy"); (2) **`CellPersistence::Overlay | Ephemeral` per grid** (default Overlay: dirty
    registered components diff into the cell overlay at unload; Ephemeral for respawning
    content) — per-grid because one world legitimately wants both. Specs: Streaming Layer 1,
    Entity Lifecycle.

---

## Deferred Ledger

Everything the resolutions above defer, consolidated so the debt is one list instead of
archaeology. Items _live_ in their home entries (pointers given); this ledger is an index, and
it must be updated whenever a resolution adds or discharges a deferral.

### A. Open questions

- **OQ 32 — Particles & particle system.** The only formally open entry. Scope framed above.

### B. Committed future design rounds — stance exists, design doesn't

Each of these is a full design session when its time comes:

| Round                | Home                                      | Scope                                                                                                                                |
| -------------------- | ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| GPU-driven phase 2   | OQ 7 (+ OQ 1 volumes tier, OQ 29 foliage) | GPU scene buffer → GPU culling → indirect draws → visibility-buffer geometry; hard additivity requirement                            |
| The Lumen analog     | OQ 4                                      | Mesh SDFs + surface cache; v2/v3 quality end-state; representation swap behind unchanged slots                                       |
| Task-graph scheduler | OQ 16 (+ OQ 6 threading notes)            | crossbeam-based, declared dependencies + priorities; unifies parallel systems, physics, streaming decode                             |
| Game-UI detail round | OQ 18 / UI Framework section              | Widget catalog, theming, text shaping (cosmic-text-class), l10n, a11y                                                                |
| Networking modules   | OQ 19                                     | Transport + replication on the preserved hooks (fixed tick, component registry, generational IDs)                                    |
| The editor           | UI Framework — Editor Consequence         | An application on the engine; consumes UI framework + reflection + aux views + pick pass; no design until there is a runtime to edit |

### C. Fully spec'd, implementation scheduled — work, not design

Deferred decals (OQ 13) · volumetric clouds, skybox interim (OQ 31) · `ReflectionProbe`
implementation (OQ 11) · serialization machinery + `EntityRef` (OQ 20) · FSR 2.2 WGSL port
(OQ 12) · predictive tick pacing — _calibration methodology is honest TBD_ (OQ 8) · HLOD
generation tooling (OQ 17/29) · device-lost recovery walk (OQ 15) · sw-6dd982 GBuffer
migration (OQ 22).

### D. Triggered contingencies — may legitimately never happen

Vertex-shader skinning fallback (OQ 14) · Surface Nets fallback if Transvoxel bites (plugin
V3) · lighting-pipeline permutations on a profiling trigger (OQ 2) · Jolt/PhysX provider swap
(OQ 16) · compute visibility-buffer volumes (OQ 1 → OQ 7) · audio middleware whole-subsystem
replacement (OQ 26) · the hexasphere sibling plugin (plugin V1) · mod mounts (OQ 27).

### E. Named-but-undesigned small items — the easiest to lose

- **Cloth simulation** — registered as a future deformer (OQ 14); zero design anywhere.
- **World-space light structure** for off-screen RT hit shading — the OQ 3 caveat;
  prerequisite for bindless sharp reflections.
- **Clip compression codec** — implementation decision under the cook pipeline (OQ 25 → OQ 27).
- **HRTF spatialization** (OQ 26) · **MetaSounds-style audio graphs** (OQ 26, v2+) ·
  **shipping telemetry** on the trace-export hook (OQ 24).
- **Edit determinism under netcode** — parked with the OQ 19 era (plugin V8).
- **Brick-size benchmark validation** — 16³ held as a constant pending profiling (plugin V7).
- **Lua-in-fixed-tick determinism rules** — required before behaviors are ever admitted to
  `fixed_update` (OQ 6 threading notes).
