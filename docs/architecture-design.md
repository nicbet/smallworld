# Smallworld Engine Architecture

## Overview

Smallworld is a hybrid game engine built from scratch in Rust on top of wgpu. It is a complete game engine — not a renderer, not a voxel engine. Voxel volumes and triangle meshes are both first-class geometry primitives sharing the same lighting model. The engine supports rasterization and raytracing simultaneously, with rasterization as the primary path and raytracing reserved for effects like shadows and global illumination.

The architecture takes the best ideas from Unreal Engine 5 — the Game Thread / Render Thread split, the Scene Proxy extraction model, composition-over-inheritance, data-driven design — and rebuilds them in idiomatic Rust without the C++ baggage. No `UObject` reflection system, no garbage collector, no `UPROPERTY` macros. Instead: trait objects for polymorphism, channels for cross-thread communication, SlotMap arenas for stable handles, and Rust's ownership system as the thread-safety guarantee.

### Design Principles

1. **Data-driven.** Components are plain data structs. Systems are functions that operate on component stores. No inheritance hierarchies, no virtual dispatch on hot paths.
2. **Thread ownership.** Each thread owns its data exclusively. Communication between threads happens via owned value-typed packets sent through channels — never shared mutable state. Sharing *immutable* data across threads (`Arc` payloads, mapped staging regions) is permitted: the rule forbids shared mutability, not sharing.
3. **Game–render firewall.** Game code never sees a `wgpu::Device`, a bind group, or a GPU buffer. The extract step is the boundary. Everything above it speaks in transforms, materials, and handles. Everything below it speaks in draw commands and GPU resources. The firewall constrains *game code*, not engine internals: engine subsystems (asset pipeline, staging pool) may create and populate CPU-visible staging resources from any thread — wgpu is internally synchronized and built for it. The narrow invariant that actually matters: **the Render Thread exclusively owns device-local resources and command submission.**
4. **Handle-based resources.** Games hold opaque handles (`AssetHandle<T>`, `ResourceHandle<T>`). Lifetime, caching, and GPU upload are engine-managed. Handles are cheap to copy and safe to hold across frames.
5. **Budget-explicit.** Frame time, GPU memory, upload bandwidth, and streaming distance are explicit budgets with engine arbitration, not emergent properties.

---

## The Frame Pipeline Architecture

### The 2-Frame Pipeline

Smallworld uses a **2-frame pipeline** where the Game Thread and Render Thread overlap by one frame. This is a deliberate simplification of UE5's 3-frame pipeline — we drop the separate RHI thread since wgpu already abstracts the graphics API.

While the Render Thread draws frame *N*, the Game Thread is already computing frame *N+1*. The extract step at the end of each game tick is the synchronization point.

- **Step 1: Game Thread (Frame N).** The engine processes game logic, physics, animation, and input. At the end of the tick, the extract step diffs the World against the `ChangeTracker` and produces a `FramePacket` — views, lights, resource operations, and per-backend scene deltas — and sends it through a bounded channel.
- **Step 2: Render Thread (Frame N, one step behind).** The Render Thread receives the `FramePacket`, applies its deltas to the retained `RenderScene`, processes GPU resource updates, then executes the render graph to produce the final image.

This introduces one frame of input latency (~16 ms at 60 fps) in exchange for up to double the throughput when both threads carry comparable load. Same tradeoff as UE5.

```
time ──────────────────────────────────────────────────────▶

Game:    │ update(N) │ extract │ update(N+1) │ extract │ ...
         │           │  send ──┐             │  send ──┐
         │           │         │             │         │
Render:  │ render(N-1)         │ render(N)             │ render(N+1)
         │                     └─recv                  └─recv
```

### Thread Model

The execution contexts, each with clear ownership boundaries:

| Context | Owns | Communicates via |
|---------|------|------------------|
| **Game Thread** (main) | World, Input, Time, Systems, AssetServer | Sends `FramePacket` (data) + lifecycle control channel (resize, quit — OQ 15) to Render Thread; receives `FrameFeedback` (N-2) |
| **Render Thread** (dedicated) | GpuContext, RenderScene (retained draw data), RenderGraph, GPU resource pools, render targets | Receives `FramePacket`; sends `FrameFeedback` back |
| **Worker Pools** (two: game + render, work-stealing) | Nothing persistent — borrow work items | Scoped tasks with `join()` / `parallel_for()`; split prevents priority inversion (OQ 16) |
| **Streaming Coordinator** (dedicated, low-priority) | Demand priority queue, budget arbiter (OQ 17) | Demand channel in (Game); `UploadBatch` channel out (Render); dispatches tasks onto the worker/IO pools |
| **Audio Thread** (dedicated) | Mixer, voices, output stream | Drains `AudioCommands` each frame |
| **IO Pool** (blocking IO + decode) | Nothing persistent | Tasks from AssetServer and Streaming Coordinator; decodes directly into staging regions (OQ 5) |

The worker pools are split (OQ 16): the **game pool** runs physics (the provider's internal parallelism binds here), animation sampling, and streaming demand computation; the **render pool** runs frustum culling, draw call sorting, and batch generation. The split prevents priority inversion — render-critical culling never queues behind physics islands — and costs little utilization because the 2-frame pipeline keeps both pools concurrently busy. A unified task-graph scheduler with declared dependencies and priorities is the v2 evolution (see Physics — Worker-Pool Split).

### Render-to-Game Feedback

The pipeline is not one-directional. The Render Thread sends a `FrameFeedback` back to the Game Thread after each frame is submitted. This travels through a separate channel — the Game Thread typically reads feedback from frame N-2 while processing frame N. Never a synchronous wait.

Feedback data has two ages. CPU-side data (cull statistics) describes the frame the feedback was sent after. GPU-derived data (timestamps, compute readbacks) is older: at submit time the GPU has not yet executed the frame, so query results are collected through a **frames-in-flight readback ring** — 2–3 buffered query/readback sets, polled via `map_async` without blocking — and each GPU-derived datum is stamped with the frame it actually measures. The Render Thread never blocks on the GPU to assemble feedback.

```
time ──────────────────────────────────────────────────────▶

Game:    │ update(N)          │ update(N+1)          │ update(N+2)
         │ reads feedback(N-2)│ reads feedback(N-1)  │ reads feedback(N)
         │           send ──┐ │            send ──┐  │
         │                  │ │                   │  │
Render:  │ render(N-1)      │ │ render(N)         │  │ render(N+1)
         │ feedback(N-1) ───┘ │ feedback(N) ──────┘  │
```

```rust
struct FrameFeedback {
    frame_index:    u64,                       // frame this feedback was sent after
    gpu_time:       Option<GpuTimingFeedback>, // from the readback ring; None until first results land
    occlusion:      OcclusionFeedback,         // CPU cull stats for frame_index
    readback:       Vec<ReadbackResult>,
}

struct GpuTimingFeedback {
    measured_frame:  u64,                 // the frame these numbers describe (≥ ring depth behind frame_index)
    total_gpu_ms:    f32,
    pass_timings:    Vec<(PassId, f32)>,  // PassId: interned pass name, assigned at graph build
    gpu_memory_used: u64,
}

struct OcclusionFeedback {
    visible_mesh_count:   u32,
    visible_volume_count: u32,
    culled_count:         u32,
}

enum ReadbackResult {
    // Generic tagged readback — pick results, exposure, debug captures. (A per-entity
    // hardware-occlusion-query variant was cut: culling is HZB-based; nothing consumed it.)
    ComputeResult { source_frame: u64, tag: u32, data: Vec<u8> },
}
```

The Game Thread accesses this through `GameContext`:

```rust
impl GameContext<'_> {
    fn feedback(&self) -> Option<&FrameFeedback>;
    fn gpu_frame_time(&self) -> f32;  // convenience: latest feedback's total_gpu_ms
}
```

Because feedback is always from a past frame, game code must treat it as advisory — useful for adaptive quality (drop LOD if GPU is overloaded), streaming priority (don't stream what's culled), and profiling, but never as ground truth for the current frame's state.

### Controlling Pipeline Depth

The 2-frame pipeline is the default. For latency-critical applications (VR, competitive multiplayer), the engine can be configured to synchronize the threads, reducing to a 1-frame pipeline at the cost of throughput:

```rust
EngineConfig {
    pipeline_mode: PipelineMode::Overlapped,  // default: 2-frame
    // PipelineMode::Lockstep               // 1-frame, lower latency
}
```

In lockstep mode, CPU-side feedback arrives from the immediately preceding frame instead of N-2 (GPU-derived data still trails by the readback ring depth), but the game thread stalls until the render thread finishes — the same bottleneck UE5 documents with `r.OneFrameThreadLag 0`.

### Frame Pacing & Latency Control

*(OQ 8 resolution, 2026-08-11.)* Three control loops, all consuming machinery that already exists — `GpuTimingFeedback`, the readback ring's completion tracking, and `ViewParams.resolution_scale`. No new cross-thread channels.

1. **GPU queue-depth throttle (v1).** The Render Thread waits on the GPU-completion signal for frame N−1 before submitting N+1, capping GPU frames in flight at `max_gpu_frames_in_flight` (default 1). Worst-case input latency becomes bounded and deterministic instead of driver-dependent — the Maximum Frame Latency analog. A correctness floor with no downside.
2. **Dynamic resolution controller (v1).** Consumes GPU frame time vs. `target_frame_time` and adjusts `resolution_scale` within `[min_scale, 1.0]`: **asymmetric response** (drop resolution fast on overrun, recover slowly), a **hysteresis band** (no oscillation around the target), **step-limited** changes (TAAU history stays valid). Strategic effect: the GPU stays inside budget, so loop 1 rarely engages and pacing stays smooth rather than reactive.
3. **Predictive tick pacing (v2 — `LatencyMode::LowLatency`, game-tunable).** Delays the game tick so input sampling happens as late as possible: a computed sleep before the INPUT phase from predicted GPU time + safety margin (Reflex-style). Tuning-sensitive — an optimistic margin misses vsync — hence flag-gated, with margins exposed to games, shipped once real content exists to calibrate against.

`PipelineMode::Lockstep` remains the blunt instrument for genuinely latency-critical applications; with these loops, Overlapped mode is latency-competitive for everything else.

```rust
struct PacingConfig {
    vsync:                    bool,
    target_frame_time_ms:     Option<f32>,  // None = display refresh interval
    max_gpu_frames_in_flight: u8,           // default 1
    drs:                      DrsConfig,
    latency_mode:             LatencyMode,
}

struct DrsConfig { enabled: bool, min_scale: f32 }

enum LatencyMode {
    Standard,    // v1: queue-depth throttle + DRS
    LowLatency,  // v2: + predictive tick pacing (game-tunable margins)
}
```

---

## The Render Thread

The Render Thread receives a `FramePacket` each frame and translates it into GPU work through a structured sequence of passes organized by a render graph.

### 1. Receive & Prepare

The Render Thread blocks on the channel until a `FramePacket` arrives. It then processes any `ResourceOp` entries — uploading new meshes and textures to GPU pools, updating material uniform data, and freeing resources for despawned entities — and applies the packet's scene deltas to the retained `RenderScene`: upserting and removing commands in the shared mesh draw store, and handing each backend's custom payload to its renderer half.

This is the only point where the Render Thread mutates its persistent state (GPU resource caches and the `RenderScene`). Everything after this is read-only traversal of the scene and packet.

### 2. Visibility & Culling

Before anything is drawn, the engine determines what each view can see. Culling is CPU-driven in v1 (worker-pool parallel); GPU-driven culling with indirect draws is the sanctioned phase-2 path (OQ 7) and must slot in additively. Culling is **per-view**: the packet's main and auxiliary views, plus shadow views the Render Thread derives from the shadow-casting lights (cascade fitting against the main view — the equivalent of UE5's InitViews shadow setup). The **engine itself culls the shared mesh store** — it owns the store; flat AABB tests, worker-pool parallel. Each registered geometry renderer culls only its *custom-lane* retained data per view via `renderer.cull()`, allowing specialized strategies (e.g., octree traversal for volumes). `MeshBackend` therefore needs no renderer half at all — its geometry lives entirely in the engine-culled shared store.

- **View setup.** Collect this frame's views: main camera, aux views (render-to-texture, probes), and one view per shadow cascade / shadowed local light.
- **RT instance collection.** When RT is enabled, the TLAS instance list is gathered **before any per-view culling**, from the retained scene under its own, larger culling domain (an RT radius around the camera — off-screen geometry must still exist for shadows, reflections, and GI). See the Ray Tracing section.
- **Frustum culling.** Test every draw command's world-space AABB against each view's frustum planes. Parallelized across the worker pool.
- **Occlusion culling (HZB).** Using the Hierarchical Z-Buffer built from the previous frame's *final opaque depth* — all backends, not just meshes (see the GBuffer stage) — test remaining objects in the main view to discard those hidden behind large occluders.
- **LOD selection.** For volumes and meshes with LOD levels, select the appropriate detail tier based on screen-space size or distance. Each backend owns its LOD strategy. Selection uses **hysteresis** — separate up/down thresholds — so boundary-distance oscillation cannot ping-pong transitions; transitions themselves use the fade/dither contract (see the Mesh Drawing Pipeline) and **gate on residency** (see Streaming).
- **Sort & batch.** Per view: opaque draws sorted front-to-back (minimize overdraw), transparent draws back-to-front. Draws sharing pipeline state are merged into instanced batches.

### 3. Deformation (`DeformPass`)

*(OQ 14 resolution, 2026-08-11.)* All GPU vertex deformation — skinning first among it — runs once per frame in a compute stage before any geometry pass. Downstream, deformed geometry is indistinguishable from static geometry.

- **Compute pre-skin (skin cache).** For each skinned instance in this frame's deformation domain — the union of the per-view cull results, **plus the RT-eligible set (inside the RT culling radius) when RT is active**, so BLAS refit never consumes stale vertices — a compute dispatch applies the bone palette and writes deformed positions/normals/tangents into a per-instance output vertex buffer. Depth pre-pass, GBuffer, every shadow view, and BLAS refit all consume that buffer — skin once, consume everywhere; **one skinning implementation serves raster and RT.**
- **Deformers are an extension point.** Skinning is the built-in deformer; morph targets, cloth, and procedural deformation register as additional compute deformers writing the same output buffers (the UE5 Deformer Graph shape). Plugin-friendly by construction.
- **Velocity via buffer aliasing — no shader permutations.** Geometry vertex shaders always read a `position_prev` attribute and multiply by `prev_world_matrix`. Rigid draws bind the *same* position buffer as `position_prev` (all motion comes from the matrix); deformed draws bind last frame's deformed output (pose motion), with the matrix carrying object motion. One shader, both cases, exact skinned motion vectors. Deformed outputs are double-buffered for this.
- **Budgeted (Principle 5).** Deformed-output memory (~40 B/vertex × 2 buffers × instance) is a named budget; over budget → LOD down or cap deformed instances. There is deliberately **no vertex-shader fallback path** — one implementation, per the permutation-as-optimization-never-architecture rule; a fallback is added only if a shipped need demonstrates it.
- **Animation sampling stays on the CPU** (worker pool): blend trees, IK, and clip evaluation are game-state logic. Bone palettes (~6 KB per character) upload per frame via the staging pool. The GPU deforms; it never runs animation logic.

### 4. Depth Pre-Pass

Establishes the scene's depth early to prevent overdraw in the full GBuffer pass.

- **Early Z.** Opaque meshes render depth-only to the Z-buffer.

The HZB is deliberately *not* built here — a pre-pass HZB would contain only mesh depth, and geometry from other backends (raymarched volumes, custom plugins) would never act as occluders. It is built after the GBuffer stage, once every backend has contributed depth.

### 5. GBuffer Pass (Geometry)

Visible opaque surfaces write their material properties to the Geometry Buffer. Smallworld uses deferred rendering, separating geometry from lighting.

| GBuffer Target | Format | Data |
|----------------|--------|------|
| Albedo | Rgba8UnormSrgb | Base color (RGB); A = shading model ID (4 bits) + flags (4 bits) |
| Normal | Rgba16Float | World-space normals (octahedral encoded) |
| Material | Rgba8Unorm | Roughness, metallic, reflectance, AO |
| Emissive | Rgba16Float | Self-illumination (RGB) + intensity |
| Velocity | Rg16Float | Per-pixel motion vectors (TAA, motion blur) |
| Depth | D32Float | Z-buffer |

Both rendering paths write to this same GBuffer:

- **Rasterized meshes** write via traditional vertex/fragment shaders through the `GBufferPass`.
- **Raymarched volumes** write via fragment-shader raymarching over rasterized proxy geometry through the `VolumePass`, exporting real depth via `frag_depth` (see the Volume Rendering Mechanism below).

The lighting pass and everything downstream never knows which path produced a given pixel. (A mesh/volume source bit exists among the albedo-alpha flag bits for debug tooling — but no lighting or post pass may branch on it.)

**Shading Model ID.** The albedo alpha channel carries a per-pixel shading model ID (up to 16 models). The lighting pass switches on it to select the lighting response — `Standard` (Cook-Torrance PBR), `Unlit`, and registered custom models (toon, foliage, …). This is UE5's per-pixel shading-model mechanism: it is what lets custom materials change *how light responds*, not just which material inputs are written.

**HZB construction.** After all opaque backends have written depth — rasterized meshes and raymarched volumes alike — the final opaque depth is downsampled into the HZB mip chain used by the next frame's occlusion culling. Building the HZB here (rather than in the depth pre-pass) means volumes and custom geometry act as occluders: a voxel mountain culls the city behind it.

#### Volume Rendering Mechanism (`VolumePass`)

*(OQ 1 resolution, 2026-08-11.)* Raymarched volume tiers render as **fragment-shader raymarching over rasterized proxy geometry** — the Teardown-proven pattern, chosen because it gets depth testing, sRGB conversion, MRT writes, and shadow-view reuse from hardware, and runs on all supported GPUs:

- **Proxy geometry.** One AABB per volume object (or streaming chunk), rasterized in the `VolumePass`. The fragment shader marches the object's brick/SVO data via hierarchical DDA; the first hit writes all GBuffer targets, velocity, and `frag_depth`. Bricks of one object are traversed inside a single invocation (first-hit termination), so only inter-*object* overlap pays overdraw.
- **Depth interop.** `frag_depth` export writes the real D32Float depth buffer; hardware depth testing resolves volume-vs-volume and volume-vs-mesh ordering. Because `frag_depth` forces late-Z (WGSL has no conservative-depth hint), the shader early-outs against `depth_mesh_copy` — a snapshot of the mesh pre-pass depth taken before the `VolumePass` (a compute copy; Depth32Float cannot be reinterpreted as R32Float in wgpu).
- **Motion vectors.** The hit point's world position is transformed by the volume's previous-frame transform and previous view-projection — the same velocity math as meshes, written in the same shader. (Rigid-motion approximation; animated voxel *content* reads as changed data, not motion.)
- **Camera inside a volume.** Rasterize proxy back faces; clamp the ray start to the near plane.
- **Shadow casting.** The same shader compiled depth-only implements `ShadowCaster::render_shadow_depth` for each shadow view.
- **Distant tiers** are extracted meshes on the shared mesh stream — no raymarching at all.

**Volume LOD transitions (Voxel Plugin design — built entirely on public contracts, OQ 10):**

- **Within the raymarched tier: distance-banded blending, clipmap-style.** LOD rings around the camera with fixed blend bands; inside a band the raymarcher samples both LOD levels and lerps density/material by the distance-derived factor. Stateless and continuous under camera motion — entirely private to the raymarch shader, no engine involvement. One time-based rider: a brick arriving *late* (residency-driven, not distance-driven) fades in over ~100–200 ms from its coarser parent, which the pinned-coarse invariant guarantees is present.
- **The extracted↔raymarched handoff: convergence + complementary dither.** Convergence is a content rule — the extracted mesh for distance D is extracted *from the same coarse brick LOD* the raymarcher samples at D, so the handoff blends two renderings of nearly the same surface. The residual is hidden by a dithered cross-fade band: extracted-tier draws use the shared-stream `fade` field; the `VolumePass` dithers complementary via the public dither convention. Zero voxel-specific engine hooks — the API-sufficiency test passes again.

**Capability-gated upgrade tier (deferred to the GPU-driven work, OQ 7).** A compute visibility-buffer variant — depth+payload packed via 64-bit atomic min/max (`Capabilities::int64_atomic_min_max`; on Metal this requires Apple M2-class "Nanite atomics") — is the sanctioned future optimization, not a rejected option. It is deferred because it needs its own coverage/binning and resolve machinery, the fragment path must exist for baseline hardware anyway, and its win is unproven for large-box proxies (Nanite's software raster exists to beat micro-triangle raster inefficiency, which volume proxies don't have). **Adoption trigger:** profiling shows the fragment path limiting on capable hardware, or variable-rate marching is needed.

#### Picking & Debug IDs

*(OQ 22 resolution, 2026-08-11.)* There is deliberately **no per-frame ID target** in the GBuffer — per-pixel IDs would pay ~4 bytes/pixel of write+read bandwidth every frame for consumers that are better served elsewhere:

- **Gameplay picking = CPU raycast** against the BVH on the Game Thread. Zero GPU involvement, zero latency, and it can hit entities the camera culled.
- **Tools/editor picking = on-demand pick pass**, scissored to a few pixels around the cursor. Each geometry path has an ID-output shader variant writing a tagged `PickId` (u32: 2-bit source tag + 30-bit payload — the mesh path writes the instance-slot index into the shared `InstanceData` buffer, which resolves uniquely to draw + instance; the volume path writes the entity index). Results return through the readback ring (~2-frame latency, fine for tools). The CPU resolves PickId → entity/material and **validates the entity generation**, so a pick landing after a despawn misses cleanly instead of hitting a recycled slot.
- **Debug views** (entity/material heatmaps) run the same pass full-screen, only while active.

Material identity has no dedicated storage anywhere — it is derivable: PickId → draw → `material_gpu_id`.

#### Deferred Decals (core feature, scheduled)

*(OQ 13 resolution, 2026-08-11.)* Decals are a **core engine feature**, implemented as standard deferred GBuffer decals: projected box volumes rendered after opaque geometry and before lighting, reading depth to reconstruct the surface and blending albedo/normal/material contributions into the existing GBuffer targets (respecting `DrawFlags::RECEIVE_DECALS`). Albedo blending is **write-masked to RGB** — the alpha channel's shading-model/flag bits are never touched. Normal blending decodes, blends, and re-encodes the octahedral normal target. The pass is purely additive over existing contracts — no new render targets, no downstream changes — which is why implementation is safely scheduled after the v1 rendering core lands without any design debt accruing in the meantime.

### 6. Shadow Pass

The engine renders depth from the perspective of each shadow-casting light into a shadow atlas, using the per-shadow-view draw lists produced during culling — shadow views see geometry the main camera culled.

Two kinds of geometry feed each shadow view:

- **The shared mesh stream.** Every `MeshDrawCommand` flagged `CAST_SHADOW` renders into the atlas — regardless of which backend emitted it (the Voxel Plugin's extracted-mesh tiers included).
- **`ShadowCaster` participants.** Backends whose geometry has no triangle form implement the `ShadowCaster` participation trait to render depth into a given shadow view themselves (e.g., raymarched volume depth).

Light types:

- **Directional lights** use cascaded shadow maps (CSM) with configurable cascade count (1–4).
- **Point and spot lights** render into atlas sub-regions.
- **Virtual shadow maps** (future) would cache static shadow pages and only re-render where dynamic objects move.

### 7. Volumetrics (Froxel Media)

*(OQ 9 resolution, 2026-08-11.)* Participating media — global fog, local fog volumes, plugin-injected media — is computed in a frustum-aligned voxel grid (**froxels**) and applied wherever depth is known. Pure compute, no capability gates. The classic four-stage pipeline (Assassin's Creed 4 / Frostbite / UE5 Volumetric Fog):

1. **Density injection.** Global exponential height fog (from `EnvironmentParams`) and `FogVolume` entities are splatted into the grid (density, albedo, emission, phase). Injection is a **public contract**: plugins and games register injector passes that add density/emission — the Voxel Plugin's far-tier smoke uses exactly this.
2. **Froxel lighting.** Per froxel: sample the clustered light grid + shadow atlas, evaluate in-scattering with a Henyey-Greenstein phase function.
3. **Temporal blend.** Reproject and blend with the previous frame's froxel volume (jittered sampling).
4. **Integration.** Front-to-back accumulation into a scattering/transmittance volume.

Consumers sample the integrated volume by pixel depth: the lighting pass applies it to all opaque pixels (meshes and raymarched volumes alike — both are in the GBuffer), the sky pass applies the far-field value, and translucent draws sample it in their forward shaders. Default grid ~160×90×64, quality-tiered, Rgba16Float. The froxel volume is **always bound** (cheap when empty), sidestepping the optional-binding question of OQ 2 for fog.

### 8. Lighting Pass (Deferred Shading)

A full-screen compute dispatch evaluates deferred shading by reading the GBuffer, shadow atlas, and light buffer. Each pixel's lighting response is selected by its GBuffer shading model ID — `Standard` is Cook-Torrance PBR; other registered models (toon, foliage, unlit) branch here.

- **Clustered light assignment.** Screen-space tiles × depth slices. Lights are assigned to clusters on the CPU. Each cluster stores up to 32 light indices.
- **Shadow evaluation.** Percentage-closer filtering (PCF) samples the shadow atlas per light.
- **Pluggable indirect inputs.** The lighting pass declares public input slots for an indirect-diffuse (GI) texture, per-light shadow masks, and **sky visibility**. Engine RT passes feed the first two when hardware RT is available — but all are a **public render-graph contract**: a plugin (e.g., the Voxel Plugin's SVO-traced GI and sky visibility) or a future software-GI tier feeds the same slots without touching the lighting pass. This is the GI upgrade path: each slot progressively supersedes the fallback below it.
- **Indirect diffuse chain** *(OQ 11, ladder completed by OQ 4)*: GI input slot when fed (RT GI → screen traces + GI clipmap → plugin GI) → sky SH9 irradiance × AO × sky visibility → constant ambient.
- **Indirect specular chain** *(OQ 11)*: RT reflections → SSR (**always-on**, not RT-gated) → GI-clipmap rough-specular cones (when the software GI tier is active — OQ 4) → local reflection probes (when present — deferred feature) → prefiltered sky cubemap × sky visibility.
- **Sky visibility is mandatory.** The environment term — specular *and* diffuse — is always modulated by a sky-visibility factor, so interiors go dark instead of sky-mirrored. Floor (core): bent-normal/SSAO-derived specular occlusion — screen-space, no authoring, works for any game. Upgrades (public slot): cone-traced visibility from the core GI clipmap when the software tier is active (OQ 4), or the Voxel Plugin's SVO-traced directional visibility — exact and destruction-proof (carve the roof open; visibility updates the same frame).
- **Fog application.** Sample the integrated froxel volume at each pixel's depth and apply scattering/transmittance (see Volumetrics).
- **Output.** HDR lighting result written to an Rgba16Float texture.

#### Software GI Tier — the GI Clipmap

*(OQ 4 resolution, 2026-08-11.)* When hardware RT GI is unavailable or disabled, core provides software GI: **screen traces first, then cone tracing against a lighting-domain voxel clipmap** — the SVOGI family, with shipped precedent in CryEngine's SVOGI carrying Kingdom Come: Deliverance 1 and 2 (open world, time-of-day, no bakes).

- **The clipmap.** Camera-centered cascaded 3D textures (opacity, albedo, normal, emissive) — a *lighting-domain* voxelization, distinct from the Voxel Plugin's content SVO. Geometry enters by **conservative rasterization of the shared mesh stream** (any backend's triangles participate automatically — a pure-mesh game gets full GI with zero content changes) or through the **GI injection point**, a participation contract in the froxel-injection mold: the Voxel Plugin injects SVO data directly — more accurate than voxelizing extracted meshes, and destruction updates GI the same frame.
- **Geometry and lighting are separate steps.** Direct light injects into voxels each frame (sun via the shadow cascades, locals via the cluster grid), so time-of-day *relights* without re-voxelizing; destruction re-voxelizes only touched clipmap regions.
- **Consumers.** Cone-traced indirect diffuse feeds the GI slot (half-res + temporal, same dispatch pattern as RT GI). **Cone-traced sky visibility** from the same structure upgrades the OQ 11 baseline for *all* games (the bent-normal floor remains beneath it). **Rough-specular cones** slot into the reflection chain between SSR and the sky term — a middle rung it previously lacked.
- **Costs, on the record.** Thin-wall light leaking is the classic VCT artifact (finer near cascades and occlusion cones mitigate it; nothing eliminates it); clipmap memory is a named budget (~100–200 MB across cascades); quality sits below Lumen-class GI.
- **World-radiance role (OQ 3).** The clipmap is also the hit-radiance source for hardware RT (GI rays always; reflection rays when rough/distant), so it is maintained whenever *either* the software tier or RT effects are active — one representation, both paths, exactly the role Lumen's surface cache plays. The v2/v3 surface cache inherits this role for both paths in the same swap.

The indirect-diffuse ladder in full: **hardware RT GI → screen traces + GI clipmap → sky SH × visibility floor** — every rung feeds the same public slots; changing rungs never touches a shader contract.

**Roadmap (v2/v3): the smallworld Lumen analog.** Mesh-distance-field + surface-cache GI — per-asset SDFs computed at import, an incrementally composited global SDF, radiance-cached surface parameterization — is the **committed quality end-state**, not a rejected option. The architecture is published and de-risked; the remaining cost is content-hardening (thin geometry, foliage, leak edge cases), not research. The upgrade is a **swap of the world representation behind the same public slots**: screen traces, temporal accumulation, and every consumer carry forward unchanged, and the clipmap likely survives as the far-field/fallback representation.

### 9. Ray Tracing (Secondary Effects)

Smallworld follows UE5's hybrid rendering model: rasterization handles primary visibility (what the camera sees), ray tracing handles secondary effects (how light bounces, reflects, and casts shadows). Rasterization writes the GBuffer; ray tracing reads it.

This section is conditional on `Capabilities::ray_query`. **The design targets wgpu's inline ray queries exclusively** — rays are traced from compute shaders via `ray_query`. wgpu exposes no ray-tracing pipelines, no shader binding table, and no hit/any-hit/intersection shaders, so nothing in this design may depend on them; on a hit, shaders receive instance/primitive indices and fetch surface data manually (see RT Global Illumination). When hardware RT is unavailable, the engine falls back to screen-space approximations (SSAO, SSR) or skips the effects entirely. The rest of the pipeline is unchanged — RT passes are optional render graph nodes.

#### Acceleration Structure

Ray tracing requires a spatial index on the GPU so rays can efficiently find intersections. This is a two-level hierarchy maintained by the Render Thread as part of `RenderState` (see Data Structures for the `AccelerationStructure` definition):

- **Bottom-Level Acceleration Structure (BLAS).** One per unique mesh geometry. Built from the vertex/index buffers already in `GpuMeshPool`. Rebuilt only when geometry changes — rare for static meshes; deformed geometry refits per frame against the `DeformPass` output buffers, the same skinned vertices the raster passes consume.
- **Top-Level Acceleration Structure (TLAS).** One per frame. References all BLAS instances with their world transforms. **Built before any per-view culling, from the retained scene** — never from a culled draw list. Rays need geometry the camera cannot see: off-screen shadow casters, the room behind the camera in a mirror. The TLAS therefore uses its own culling domain — an **RT culling radius** around the camera, larger than any view frustum (the same reason UE5 has a separate ray-tracing culling radius). `TlasContributor` backends add their instances here.

The TLAS build is a GPU operation — the Render Thread records it as a command before the RT passes execute. Cost is proportional to instance count, not triangle count, so it scales well.

Volume geometry has no triangle representation to feed into BLAS construction. Two strategies:

- **Extracted mesh BLAS.** The Voxel Plugin's extracted-mesh LOD tiers enter the TLAS like any other mesh draws — the shared mesh stream at work. Works today, coarser than the actual voxel data.
- **SVO compute raymarching (no RT hardware needed).** For voxel shadows and GI, the Voxel Plugin traces its own SVO directly in compute — no acceleration structure, no `ray_query` required — and feeds the results into the lighting pass's public shadow-mask / GI input slots. Hardware traversal of the SVO via custom intersection shaders is **not possible under wgpu** (no ray-tracing pipelines), so compute-side SVO tracing is the plugin's path. Direction agreed in principle; specifics tracked in Open Questions.

#### RT Passes

RT passes are standard `RenderPass` implementations that read the GBuffer and trace rays against the TLAS. They write to dedicated targets consumed by the lighting pass.

##### RT Shadows (`RTShadowPass`)

For each pixel in the GBuffer, cast one shadow ray toward each light source through the TLAS. Produces a per-light shadow mask — a binary (or soft-penumbra) occlusion value per pixel. Replaces or supplements the rasterized shadow atlas for lights that opt in.

- **Input:** GBuffer (depth, normal, position reconstructed from depth), TLAS, light buffer.
- **Output:** `rt_shadow_mask` — Rgba8Unorm; channels are assigned to the (up to 4) most significant RT-shadowed lights per cluster via the clustered light grid; lights beyond that fall back to the shadow atlas.
- **Dispatch:** Full-screen compute, 8×8 workgroups. One ray per pixel per light. Denoised temporally.

##### RT Global Illumination (`RTGIPass`)

Indirect lighting from light bounces. Cast rays outward from each GBuffer pixel based on a cosine-weighted hemisphere around the surface normal. There are no hit shaders under wgpu — `ray_query` returns instance and primitive indices on a hit — so hit-point radiance comes from **sampling the GI clipmap at the hit position** *(OQ 3 resolution, 2026-08-11)*: the lit clipmap is the engine's world-radiance representation, serving hardware RT and software GI alike (the Lumen surface-cache role at voxel fidelity). Coarse radiance is fine here — the cosine integration blurs it regardless. **Rule: the GI clipmap is maintained whenever either the software GI tier *or* hardware RT effects are active.**

- **Input:** GBuffer, TLAS, GI clipmap (hit radiance).
- **Output:** `rt_gi` — Rgba16Float, indirect diffuse irradiance per pixel.
- **Dispatch:** Half-resolution (one ray per 2×2 quad), spatially and temporally denoised, then upsampled. Full-resolution GI is too expensive for real-time; the denoiser fills in.

##### RT Reflections (`RTReflectionPass`)

For pixels with low roughness, cast a reflection ray based on the GBuffer normal. Hit radiance is hybrid by ray character *(OQ 3)*: **rough or distant reflections sample the GI clipmap** (v1 — zero extra machinery); **sharp near reflections upgrade to bindless hit-shading** later — vertex/material fetch via binding arrays (capability-gated: `BUFFER_BINDING_ARRAY` / `TEXTURE_BINDING_ARRAY`), texture LOD via ray cones, with the noted caveat that off-screen hit points cannot use the view-space cluster grid and need a world-space light structure (or sun + IBL only) — deferred until sharp mirror quality demands it.

- **Input:** GBuffer (normal, roughness, depth), TLAS, GI clipmap (hit radiance).
- **Output:** `rt_reflections` — Rgba16Float, reflected radiance per pixel.
- **Dispatch:** Selective — only pixels below a roughness threshold. SSR runs regardless (it is always-on in the specular chain); RT results replace SSR where rays were traced. Rough surfaces and non-RT hardware resolve through SSR → probes → sky × visibility (see the Lighting Pass specular chain).

#### Compositing into Lighting

The `LightingPass` is extended to read the RT targets when they exist:

```
LightingPass reads:
    GBuffer (albedo, normal, material, depth)     — always
    shadow_atlas                                   — always (rasterized shadows)
    rt_shadow_mask                                 — when RTShadowPass ran
    rt_gi                                          — when RTGIPass ran
    rt_reflections                                 — when RTReflectionPass ran
    clustered_light_grid                           — always
```

When RT shadows are available for a light, they replace the shadow atlas sample for that light. RT GI adds to the ambient/indirect term. RT reflections replace or blend with the specular term based on roughness.

Optional RT inputs bind through the render graph's **optional-input-slot mechanism** (see Render Graph — Optional Input Slots): always-bound neutral dummies plus per-frame uniform flags — one lighting pipeline, uniform branching, no shader permutations.

This is pure additive integration — the rasterization pipeline produces a complete image on its own. RT passes improve quality when available but nothing breaks without them.

#### Render Targets (RT)

```rust
// Added to RenderTargets when Capabilities::ray_query is true
struct RTTargets {
    shadow_mask:    wgpu::Texture,  // Rgba8Unorm — per-light RT shadow
    gi:             wgpu::Texture,  // Rgba16Float — indirect diffuse
    reflections:    wgpu::Texture,  // Rgba16Float — specular reflections
    history_shadow: wgpu::Texture,  // Rgba8Unorm — temporal accumulation for denoiser
    history_gi:     wgpu::Texture,  // Rgba16Float — temporal accumulation for denoiser
    history_refl:   wgpu::Texture,  // Rgba16Float — temporal accumulation for denoiser
}
```

#### Fallback Path (No Hardware RT)

When `ray_query` is unavailable, the engine uses screen-space approximations in the same render graph slots:

| RT Pass | Fallback | Quality tradeoff |
|---------|----------|------------------|
| `RTShadowPass` | Shadow atlas only (rasterized CSM/atlas) | No soft penumbra from RT, same shadows as baseline |
| `RTGIPass` | Screen traces + GI clipmap (software tier — OQ 4); SSAO + sky floor beneath | Coarser bounce, VCT leak artifacts vs. RT |
| `RTReflectionPass` | SSR (screen-space reflections) | Misses off-screen reflections |

If the RT passes aren't registered (because `ray_query` is false), the lighting pass's RT input slots go unfed and the fallback terms are used.

The fallback ladder no longer cliffs *(updated by OQ 4)*: below hardware RT sits the **core software GI tier** — screen traces + cone tracing against the lighting-domain GI clipmap (see the Lighting Pass) — which assumes no particular scene structure: geometry enters by rasterizing the shared mesh stream. The SSAO + sky-IBL × visibility floor remains beneath it for minimal hardware. Voxel-heavy games improve the tier further through the GI injection point (the Voxel Plugin injects SVO data directly), and all of it flows through the same public input slots.

### 10. Sky & Atmosphere

Rendered into the HDR target where depth equals the far plane. Atmosphere scattering, procedural sky, or skybox cubemap. Applies the froxel volume's far-field scattering for atmospheric consistency with fogged geometry.

#### Environment Capture & IBL

*(OQ 11 resolution, 2026-08-11.)* The sky is also the engine's image-based light source. The environment pipeline maintains three artifacts:

- **Prefiltered specular cubemap.** The sky (procedural or HDRI, per `EnvironmentParams::sky`) is captured to a cubemap and GGX-prefiltered into a roughness → mip chain.
- **SH9 irradiance.** The same capture projected onto 9 spherical-harmonic coefficients — the diffuse ambient term.
- **Split-sum BRDF LUT.** Generated once at startup.

Captures update amortized (one face or one prefilter mip per frame), so time-of-day costs a bounded slice of the frame budget. Consumers: the lighting pass (indirect chains), forward transparents (same prefiltered set), and the froxel lighting's ambient term. Local `ReflectionProbe`s (deferred — see Core Engine Components) reuse this exact capture/prefilter machinery at probe positions via aux views.

### 11. Transparency

*(OQ 9 resolution, 2026-08-11.)* Objects with alpha blending render in a forward pass over the completed opaque image (lighting + sky). They cannot write to the GBuffer.

- **Clustered Forward+ lighting.** Transparent fragments shade with the same Cook-Torrance BRDF, the same clustered light grid, the same shadow atlas, and the same registered shading models as the deferred path — one light structure, two consumers, so lighting matches across the opaque/transparent boundary. The specular environment term is the same prefiltered sky set × sky visibility (see Environment Capture & IBL).
- **Refraction.** Glass and water sample `scene_color_copy` — an HDR snapshot taken after lighting + sky. A pass cannot sample the target it blends into.
- **Fog.** Each transparent fragment samples the integrated froxel volume at its depth.
- **Sorting.** Back-to-front per draw. No OIT in v1 — interpenetration artifacts are accepted (the shipped-game norm); weighted-blended OIT is a future quality knob.
- **Translucent media ≠ surface transparency.** Smoke/fire-like media renders in this stage but through different machinery: near-field hero media in plugin-owned raymarch passes (front-to-back march lit by the cluster grid + shadow atlas, composited by transmittance against scene depth — reads `depth`, never writes it), far-field media injected into the froxel grid. The opaque `VolumePass` is never used for media.

### 12. Post-Processing

*(OQ 12 resolution, 2026-08-11.)* **Internal render resolution and display resolution are separate, first-class concepts.** All scene targets (GBuffer, HDR, froxels) allocate at internal resolution; the temporal resolve upscales; everything after it runs at display resolution. Dynamic resolution scaling is reserved in the contract: targets allocate at *maximum* internal resolution and render at a per-frame scale carried in `ViewParams` — the control loop that drives the scale is frame pacing's job (OQ 8).

Pass order: auto-exposure histogram → temporal resolve/upscale → bloom → tone mapping → color grading → dev UI. (The histogram reads the *pre-upscale* internal-res HDR buffer; its exposure output feeds both the temporal resolve and tone mapping.)

- **Temporal resolve & upscale (TAAU).** TAA and upscaling are one pass: jittered internal-res samples accumulate into a display-res history via motion-vector reprojection. Native-res TAA is TAAU at scale 1.0 — one code path, not two. The resolve is a **replaceable render-graph node** with declared inputs (HDR color, depth, velocity, exposure, jitter sequence) — exactly the interface vendor upscalers expect. Roadmap: **FSR 2.2 (WGSL port) in v2** through this slot; **DLSS when wgpu support is practical** (third-party integration crates like `dlss_wgpu` exist today; NVIDIA + Vulkan only).
- **Auto-exposure.** Histogram-based: a compute reduction over the *pre-upscale, internal-res* HDR buffer (same statistics, fewer pixels); average within percentile clamps — outlier-proof metering, a sun pixel or black corner can't hijack it; **asymmetric adaptation speeds** (dark-adaptation slower, matching eyes); EV compensation and metering mask as artist controls. `Exposure::Manual { ev }` is an explicit mode for cinematic control; the mode lives per-camera (`Camera::exposure`). The current exposure value rides `FrameFeedback` as an advisory (night-vision-style gameplay uses).
- **Bloom.** Downsample bright regions (thresholds in exposed space), blur, composite back.
- **Tone mapping.** HDR → SDR/display HDR via **ACES** (default filmic transform).
- **Color grading.** Final LUT application.
- **Dev/debug UI.** egui renders as a final render-graph pass over the post-processed image (dev tooling — OQ 18).

### 13. Present

The final image is blitted to the swapchain surface. The Render Thread loops back to receive the next `FramePacket`.

---

## Customizing the Render Pipeline

Smallworld's rendering architecture is modular at five levels, mirroring UE5's customization depth but expressed in Rust traits rather than C++ inheritance. The guiding principle: **the engine's own voxel volume support ships as the Voxel Plugin — a geometry backend plugin, not a special case.** It uses the same backend traits a game would use to add GPU particles, procedural terrain, or SDF shapes. If the API isn't powerful enough for voxels, it isn't powerful enough for games.

### 1. Custom Geometry Types (`GeometryExtractor` + `GeometryRenderer`)

The deepest customization point. A geometry backend defines a new kind of renderable — its game-side component, how it extracts into scene deltas, how the render side retains and culls that data, which GPU resources it needs, and which render passes it contributes.

A backend is **two objects, one per side of the thread firewall** — the same split as UE5's `UPrimitiveComponent` / `FPrimitiveSceneProxy`. The extractor lives on the Game Thread and never sees GPU types. The renderer lives on the Render Thread, owns its retained scene data, and may use wgpu directly — it *is* render-side code; Principle 3 constrains game code, not render plugins.

```rust
// Game Thread half — reads the World, emits deltas. No GPU types anywhere.
trait GeometryExtractor: Send {
    fn name(&self) -> &str;

    // Which component type does this extractor process?
    fn component_id(&self) -> TypeId;

    // Diff the World against the change tracker; write mesh-draw updates and/or a
    // backend-specific delta payload for the renderer half. View-independent.
    fn extract(&mut self, world: &World, changes: &ChangeTracker, out: &mut SceneDeltaWriter);
}

// Render Thread half — owns retained scene data for this geometry type.
trait GeometryRenderer: Send {
    fn name(&self) -> &str;

    // Apply this frame's delta payload to retained state; upload/update GPU resources.
    fn apply_delta(&mut self, delta: Box<dyn Any + Send>, state: &mut RenderState);

    // Cull this backend's CUSTOM-LANE retained data for one view. (The shared mesh store
    // is culled by the engine itself — see Visibility & Culling.)
    fn cull(&self, view: &ViewParams, hzb: Option<&wgpu::TextureView>, out: &mut ViewDrawList);

    // Register the render passes this geometry type contributes.
    fn register_passes(&self, graph: &mut RenderGraph);

    // Optional pass participation (see below).
    fn shadow_caster(&self) -> Option<&dyn ShadowCaster> { None }
    fn tlas_contributor(&self) -> Option<&dyn TlasContributor> { None }
}

// Per-view culling output. `mesh_draws` is filled by the ENGINE's shared-store culling;
// `custom` is appended by each backend's cull() for its own passes to downcast.
struct ViewDrawList {
    mesh_draws: Vec<DrawId>,               // indices into the shared mesh store
    custom:     Vec<Box<dyn Any + Send>>,  // downcast by the owning backend's passes
}
```

#### The Shared Mesh Stream (participation contract #1)

`SceneDeltaWriter` gives every extractor two lanes:

```rust
impl SceneDeltaWriter {
    // Shared lane: standard mesh draws. Anything written here automatically participates
    // in the depth pre-pass, shadow atlas, HZB, TLAS, and velocity — the engine's passes
    // all consume the shared mesh store.
    fn upsert_mesh_draw(&mut self, id: DrawId, cmd: MeshDrawCommand);
    fn remove_mesh_draw(&mut self, id: DrawId);

    // Backend lane: opaque payload delivered to this backend's renderer half.
    fn custom(&mut self, payload: impl Any + Send);
}
```

The shared mesh stream is smallworld's equivalent of UE5's `FMeshBatch` common currency: it is *why* custom geometry gets shadows, occlusion, and RT presence for free. A backend that can express its geometry as triangles — even coarsely — should. The Voxel Plugin's extracted-mesh LOD tiers flow through this lane; only its raymarched near-field detail needs the custom lane.

#### Pass-Participation Traits (participation contract #2)

Geometry with no triangle form participates in engine passes through explicit traits:

```rust
trait ShadowCaster {
    // Render this backend's depth into one shadow view.
    fn render_shadow_depth(&self, view: &ViewParams, ctx: &mut PassContext);
}

trait TlasContributor {
    // Contribute BLAS instances to the frame's TLAS build (pre-cull, RT culling radius).
    fn tlas_instances(&self, out: &mut TlasInstanceList);
}
```

The set is intentionally small and grows only when a real backend needs a new integration point.

#### Built-in Backends

The engine ships two backends, using the same traits a game would. `MeshBackend` is the degenerate case: extractor-only — its geometry lives entirely in the engine-culled shared store, so it needs no renderer half.

| Backend | Component | Shared mesh stream | Custom lane | Own passes | Shadow / RT participation |
|---------|-----------|--------------------|-------------|------------|---------------------------|
| `MeshBackend` | `MeshRenderer` | All draws | — | — (engine passes consume the shared store: `DepthPrepass`, `GBufferPass`, `ShadowPass`, `TransparencyPass`) | Via shared stream |
| Voxel Plugin (`VolumeBackend`) | `VolumeRenderer` | Extracted-mesh LOD tiers | Brick/SVO residency + raymarch data | `VolumePass` | Shared stream (extracted tiers) + `ShadowCaster` for raymarched detail |

Both converge at the same GBuffer — the lighting pass and everything downstream is backend-agnostic.

#### Registering a Custom Backend

Games register backends at init time. The extractor stays on the Game Thread; the renderer half is moved to the Render Thread once, at registration. The engine integrates them into the extract → apply → cull → render pipeline automatically.

```rust
impl GameContext<'_> {
    fn register_geometry_backend(
        &mut self,
        extractor: impl GeometryExtractor + 'static,
        renderer: impl GeometryRenderer + 'static,
    );
}

// Example: a game adds GPU particle rendering
struct ParticleExtractor { /* ... */ }
struct ParticleRenderer  { /* retained emitter GPU state ... */ }

impl GeometryExtractor for ParticleExtractor {
    fn name(&self) -> &str { "particles" }
    fn component_id(&self) -> TypeId { TypeId::of::<ParticleEmitter>() }
    fn extract(&mut self, world: &World, changes: &ChangeTracker,
        out: &mut SceneDeltaWriter) { /* ... */ }
}

impl GeometryRenderer for ParticleRenderer {
    fn name(&self) -> &str { "particles" }
    fn apply_delta(&mut self, delta: Box<dyn Any + Send>, state: &mut RenderState) { /* ... */ }
    fn cull(&self, view: &ViewParams, hzb: Option<&wgpu::TextureView>,
        out: &mut ViewDrawList) { /* ... */ }
    fn register_passes(&self, graph: &mut RenderGraph) { /* ... */ }
}
```

### 2. Custom Draw Processing (`DrawProcessor`)

If you want to modify how a standard pass handles draws — custom sorting, per-draw filtering, shader binding overrides — without writing an entire pass from scratch, you provide a `DrawProcessor`.

A `DrawProcessor` operates on the **shared mesh stream** — every `MeshDrawCommand`, regardless of which backend emitted it. Custom-lane geometry is processed by its owning backend's own passes and is not visible here.

```rust
trait DrawProcessor: Send + Sync {
    fn name(&self) -> &str;

    // Filter: return false to exclude a draw from this pass
    fn filter(&self, command: &MeshDrawCommand, pass: &str) -> bool;

    // Sort: custom sort key for draw ordering within a pass
    fn sort_key(&self, command: &MeshDrawCommand, pass: &str) -> u64;

    // Bind: inject custom bind groups or push constants before a draw
    fn bind(&self, command: &MeshDrawCommand, pass: &str, ctx: &mut PassContext);
}

impl GameContext<'_> {
    fn set_draw_processor(&mut self, pass: &str, processor: impl DrawProcessor + 'static);
}
```

This is the equivalent of UE5's `FMeshPassProcessor` — it lets you modify draw behavior per-pass without replacing the pass itself.

### 3. Pipeline Injection (`RenderGraph::add_pass`)

Add entirely new passes to the frame without modifying engine source. Already defined in the Render Graph section — games call `graph.add_pass()` with a custom `RenderPass` implementation.

### 4. Custom Passes & Shaders (`RenderPass`)

Write full compute or raster passes with custom shaders. The `RenderPass` trait gives you access to the `CommandEncoder` and all render state. The render graph handles resource dependencies and barriers.

### 5. Custom Materials (Shader Composition)

Games need materials beyond the built-in PBR model — toon shading, water, foliage wind, hologram effects. Custom materials customize two things independently:

1. **What goes into the GBuffer** — a WGSL fragment computes the albedo, normal, roughness, etc.
2. **How light responds to it** — a shading model ID, written per-pixel into the GBuffer and switched on in the lighting pass.

Toon shading is a shading model; wet-surface albedo is a fragment. Both still write to the same GBuffer.

```rust
struct CustomMaterial {
    base:            MaterialDef,            // PBR properties still available
    shading_model:   ShadingModel,           // lighting response, written per-pixel to the GBuffer
    fragment_shader: ShaderFragment,         // custom WGSL fragment
    uniforms:        Vec<(String, UniformValue)>,  // custom uniform data
    textures:        Vec<(String, AssetHandle<TextureAsset>)>,
}

enum ShadingModel {
    Standard,    // Cook-Torrance PBR (default for all non-custom materials)
    Unlit,
    Toon,
    Foliage,
    // …engine-registered custom models, ≤ 16 total (4 GBuffer bits)
}

struct ShaderFragment {
    source: String,          // WGSL code
    entry_point: String,     // function name
    stage: ShaderStage,      // Fragment, Vertex, or Compute
}
```

The engine composes the final shader by concatenating the standard GBuffer output code with the custom fragment. Custom materials control *how* the GBuffer inputs are computed and *which lighting response* consumes them — not where they go. Lighting behavior beyond the registered shading models requires a custom pass (level 3/4).

---

## Data Structures

### 1. Extract & the Retained Scene (Bridging the Threads)

Smallworld adopts the same retained-scene principle as UE5's proxy system: the Render Thread owns a persistent **`RenderScene`** — the shared mesh draw store plus each backend renderer's retained data — and the Game Thread sends **deltas**, not snapshots. The difference from UE5 is the transport: deltas cross the boundary as owned values through a channel, never as writes into shared memory. No shared ownership, no lifetime entanglement — and no re-sending the world every frame. A static entity costs zero extract work and zero transfer after its first frame; this is what makes `EntityFlags::STATIC` meaningful.

#### FramePacket

The per-frame message from Game to Render. Everything in it is owned and `Send`.

```rust
struct FramePacket {
    frame_index:  u64,
    views:        Vec<ViewParams>,     // main camera + game-defined aux views (RTT, probes)
    lights:       Vec<LightParams>,    // small; re-sent in full each frame
    environment:  EnvironmentParams,
    resource_ops: Vec<ResourceOp>,
    mesh_delta:   MeshDrawDelta,       // shared mesh stream updates (all backends)
    deltas:       Vec<(BackendId, Box<dyn Any + Send>)>,  // per-backend custom payloads
}

struct MeshDrawDelta {
    upserts:          Vec<(DrawId, MeshDrawCommand)>,     // spawned or changed draws
    removes:          Vec<DrawId>,                        // despawned draws
    instance_upserts: Vec<(InstanceSlot, InstanceData)>,  // transforms, fade — the hot lane
    instance_removes: Vec<InstanceSlot>,
}
```

- **Owned and `Send`.** No references, no borrowed lifetimes. Once sent through the channel, the Game Thread is free to mutate the World.
- **Delta-driven.** The extract step walks the `ChangeTracker`'s dirty sets; unchanged entities produce nothing. The retained `RenderScene` carries everything else across frames.
- **The instance lane is the hot path.** `DrawId`s and `InstanceSlot`s are allocated game-side by the extract layer; a slot is stable for an instance's lifetime — the `PickId` contract depends on that stability. Transform changes and transition fades ride `instance_upserts` without touching commands; a moving entity costs one `InstanceData` write per frame, not a command rebuild.
- **Read-only extraction.** The extract functions borrow `&World`, never `&mut World`.
- **Extensible.** Each registered extractor writes into the shared mesh lane and/or its custom lane. The engine doesn't know what's inside a custom payload — it's opaque until the matching renderer half applies it.
- **Shadow views are not in the packet.** The Render Thread derives them from `lights` + the main view during culling.

#### ViewParams

```rust
struct ViewParams {
    kind:            ViewKind,
    view:            Mat4,
    projection:      Mat4,
    view_projection: Mat4,
    position:        Vec3,
    frustum_planes:  [Vec4; 6],
    near:            f32,
    far:             f32,
    jitter:          Vec2,       // TAA sub-pixel jitter (main view only)
    resolution_scale: f32,       // dynamic-resolution scale for internal-res targets (1.0 = full; OQ 12)
}

enum ViewKind {
    Main,
    Aux { target: RenderTargetRef },          // render-to-texture, probes, split-screen
    Shadow { light: LightId, cascade: u8 },   // derived render-side, never sent in the packet
}
```

#### ResourceOp

When the game adds, modifies, or removes assets, the extract step encodes these as resource operations for the Render Thread.

```rust
enum ResourceOp {
    UploadMesh     { gpu_id: GpuId, vertices: StagingRef, indices: StagingRef, bounds: AABB },
    UploadTexture  { gpu_id: GpuId, staging: StagingRef, width: u32, height: u32,
                     format: TextureFormat, mip_count: u32 },  // staging holds all mips, row-pitch aligned
    UpdateMaterial { gpu_id: GpuId, props: MaterialGpuProps },  // small: stays by-value
    Free           { gpu_id: GpuId, kind: ResourceKind },
}

// Handle into the engine-owned staging pool: a mapped wgpu buffer region populated
// off-thread by the asset pipeline. The Render Thread records a GPU copy from it and
// the region returns to the pool once that submission's fence completes.
struct StagingRef {
    buffer: StagingBufferId,
    offset: u64,
    size:   u64,
}
```

#### Staging Pool & Upload Path

*(OQ 5 resolution, 2026-08-11.)* Bulk asset bytes never travel by value and are never memcpy'd on a hot thread. The engine owns a **staging pool**: CPU-visible mapped `wgpu` buffers, ring/size-class allocated, fence-reclaimed, and budgeted like every other pool (Principle 5 — wgpu's internal `write_*` staging would be invisible memory; ours is accounted).

- **Decode-direct population.** Asset IO/decode threads write decoder output *straight into* a mapped staging region (rows 256-byte aligned at decode time; sequential writes — it's write-combined memory). This is the write the decoder performs anyway; no thread performs an additional payload copy.
- **O(1) render-thread cost.** The Render Thread records `copy_buffer_to_buffer` / `copy_buffer_to_texture` from staging into the device-local pools — command recording only, independent of payload size. (The alternative — `Arc` bytes + `queue.write_*` — would put an O(bytes) memcpy on the Render Thread per upload; rejected for steady-state streaming workloads.)
- **Firewall-clean.** Creating and mapping staging buffers off-thread is designed-for wgpu usage (`Device`/`Queue` are internally synchronized). This is engine-internal machinery; Principle 3 constrains game code, and the Render Thread's exclusive ownership of *device-local* resources and submission is untouched.
- **Small payloads stay by-value.** `UpdateMaterial` uniforms and other sub-threshold payloads ride the channel directly — pool overhead isn't worth it. `Arc` of immutable bytes remains legal engine-internal transport where staging doesn't fit (e.g., CPU-retained asset caches).
- **Shared with streaming.** This pool is the same subsystem the out-of-core brick streaming path rides (OQ 17) — one system, two clients; brick uploads use dedicated rings within it, not generic `ResourceOp`s.
- **Teardown.** The pool participates in the device teardown protocol (OQ 15): in-flight mapped regions drain before device destruction.

### 2. The Mesh Drawing Pipeline

#### MeshDrawCommand

The render-ready description of a single draw. Fully resolved — no handles to chase, no indirection. Instancing is first-class: a command draws `instances.len()` copies; a single-instance draw is the degenerate case.

```rust
struct MeshDrawCommand {
    mesh_gpu_id:     GpuId,        // index into GpuMeshPool
    material_gpu_id: GpuId,        // index into GpuMaterialPool
    instances:       Range<u32>,   // slice of the shared InstanceData buffer; len ≥ 1
    bounds:          AABB,         // world-space union over instances, for culling
    flags:           DrawFlags,    // shadow casting, double-sided, alpha mode
}

// One entry per instance, in a shared, GPU-visible buffer
struct InstanceData {
    world_matrix:      Mat4,
    prev_world_matrix: Mat4,       // for motion vectors
    fade:              f32,        // 1.0 = fully present; < 1.0 = dithered LOD transition (OQ 10)
    flags:             u32,        // bit 0: dither complement — inverts the screen-door pattern
}
```

This is the equivalent of UE5's `FMeshDrawCommand` — a fully stateless draw description that can be sorted, merged, and cached. Because the render side retains the mesh store across frames, static commands genuinely *are* cached: sorted batch lists for static geometry are rebuilt only when the store changes, not per frame. Unlike UE5, we don't have the intermediate `FMeshBatch` layer as a data structure — its cross-backend role is played by the shared mesh stream itself; the extract step produces final draw commands directly.

#### LOD Transitions — the Fade/Dither Contract

*(OQ 10 resolution, 2026-08-11 — core mechanism; transition policy belongs to each backend.)* Every pass that consumes the shared mesh stream (depth pre-pass, GBuffer, shadows) honors `InstanceData.fade` via **screen-door dithering**: a fragment is discarded when the dither threshold for its screen position exceeds `fade`, with the complement flag inverting the pattern. TAA resolves the stipple into a smooth cross-fade. A mesh LOD transition is therefore two temporary draws in the retained store — outgoing LOD fading out, incoming LOD fading in with the complement bit — upserted at transition start and collapsed to one when done (~150–300 ms window). Each screen pixel shows exactly one LOD at any instant, so depth and GBuffer stay consistent.

**The dither convention (pattern + fade→threshold mapping) is public contract, not an engine internal** — plugin-owned passes must be able to dither *complementary to* shared-stream draws (the Voxel Plugin's tier handoff depends on this). Hard switches were tested and rejected (visible popping); geomorphing was tested and rejected (authoring-fragile, poor results unless perfect).

#### VolumeDrawCommand

The Voxel Plugin's custom-lane draw data — carried in its backend delta payload and consumed only by `VolumePass`. (Its extracted-mesh tiers travel the shared mesh stream as ordinary `MeshDrawCommand`s.)

```rust
struct VolumeDrawCommand {
    volume_id: EntityId,
    bounds:    AABB,
    lod_level: u8,        // demand hint: the LOD the game side wants — never residency
}
```

*(OQ 21, 2026-08-11: the former `brick_residency` field is gone. Residency truth lives with the brick pool on the streaming side — the game thread structurally cannot know it (feedback is ≥2 frames stale by design). Draw data carries demand only; `VolumePass` reads residency from the pool it renders from and falls back to coarser SVO parents for not-yet-resident bricks — the virtual-texturing residency pattern.)*

#### LightParams

```rust
struct LightParams {
    kind:             LightKind,
    position:         Vec3,
    direction:        Vec3,
    color_intensity:  Vec4,       // rgb * intensity
    radius:           f32,
    shadow:           Option<ShadowConfig>,
}
```

#### EnvironmentParams

*(Defined as part of the OQ 11 resolution; carries the OQ 9 height-fog rider.)*

```rust
struct EnvironmentParams {
    sky:        SkyMode,
    ambient:    AmbientMode,
    height_fog: HeightFogParams,
}

enum SkyMode {
    Procedural { turbidity: f32, ground_albedo: Vec3 },  // driven by the sun directional light
    Cubemap    { texture: AssetHandle<TextureAsset> },   // authored HDRI
    Color      (Vec3),                                   // flat (debug / stylized)
}

enum AmbientMode {
    Sky,             // SH9 irradiance projected from the sky capture
    Constant(Vec3),
}

struct HeightFogParams {
    density:        f32,
    height:         f32,   // fog base height (world Y)
    falloff:        f32,   // exponential falloff with altitude
    inscatter:      Vec3,  // fog color / inscatter tint
    start_distance: f32,
}
```

#### DrawFlags

```rust
bitflags! {
    struct DrawFlags: u8 {
        const CAST_SHADOW    = 0x01;
        const RECEIVE_SHADOW = 0x02;
        const DOUBLE_SIDED   = 0x04;
        const ALPHA_MASK     = 0x08;
        const TRANSPARENT    = 0x10;
        const RECEIVE_DECALS = 0x20;  // reserved now; consumed by the deferred decal pass (OQ 13)
    }
}
```

### 3. Render Thread Resources

The Render Thread owns all GPU memory through typed pools. Resources are identified by `GpuId` — an opaque **generational dense index** that the extract layer maps from game-side handles. Pool lookups on the draw path are array indexing, never hashing.

```rust
struct GpuId { index: u32, generation: u32 }
```

When hardware RT is available, the Render Thread also maintains the TLAS/BLAS acceleration structure.

#### GpuContext

```rust
struct GpuContext {
    instance: wgpu::Instance,
    adapter:  wgpu::Adapter,
    device:   wgpu::Device,
    queue:    wgpu::Queue,
    surface:  wgpu::Surface,
    caps:     Capabilities,
}
```

#### Capabilities

Probed at startup. The engine adapts its feature set based on what the hardware supports.

```rust
struct Capabilities {
    timestamp_query:      bool,
    ray_query:            bool,
    mesh_shader:          bool,
    shader_f16:           bool,
    subgroups:            bool,
    int64_atomic_min_max: bool,  // 64-bit atomic min/max (Metal: Apple M2-class+ "Nanite atomics")
    texture_int64_atomic: bool,  // R64Uint image atomic min/max (MSL 3.1+)
    max_buffer_mb:        u32,
    max_texture_dim:      u32,
    min_ubo_alignment:    u32,
}
```

#### Acceleration Structure (RT)

Allocated only when `Capabilities::ray_query` is true. Maintained by the Render Thread — the TLAS is rebuilt each frame from the retained scene, **before per-view culling**, within the RT culling radius (see the Ray Tracing section). This is the canonical definition.

```rust
struct AccelerationStructure {
    blas_cache: SecondaryMap<GpuId, BlasEntry>,  // parallel to GpuMeshPool — indexed, not hashed
    tlas:       wgpu::Tlas,
    tlas_dirty: bool,
}

struct BlasEntry {
    blas:        wgpu::Blas,
    mesh_gpu_id: GpuId,
    generation:  u32,         // rebuilt when mesh geometry changes
}
```

#### GPU Resource Pools

```rust
struct GpuMeshPool {
    meshes: SlotMap<GpuId, GpuMesh>,   // dense, generational — O(1) indexed lookup on the draw path
    budget: MemoryBudget,
}

struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer:  wgpu::Buffer,
    index_count:   u32,
    vertex_count:  u32,
    bounds:        AABB,
}

struct GpuTexturePool {
    textures: SlotMap<GpuId, GpuTexture>,
    budget:   MemoryBudget,
}

struct GpuTexture {
    texture: wgpu::Texture,
    view:    wgpu::TextureView,
    sampler: wgpu::Sampler,
    width:   u32,
    height:  u32,
    format:  wgpu::TextureFormat,
}

struct GpuMaterialPool {
    materials: SlotMap<GpuId, GpuMaterialEntry>,
}

struct GpuMaterialEntry {
    uniform_offset: u32,            // offset into material UBO
    texture_bind_group: wgpu::BindGroup,
}
```

#### Render Targets

```rust
struct RenderTargets {
    // Scene targets allocate at MAXIMUM internal resolution; DRS renders at
    // ViewParams.resolution_scale via viewport. Post-upscale targets are display-res. (OQ 12)
    // Core
    depth:            wgpu::Texture,  // D32Float
    depth_mesh_copy:  wgpu::Texture,  // R32Float — mesh pre-pass depth snapshot (VolumePass early-out)
    gbuffer_albedo:   wgpu::Texture,  // Rgba8UnormSrgb
    gbuffer_normal:   wgpu::Texture,  // Rgba16Float
    gbuffer_material: wgpu::Texture,  // Rgba8Unorm
    gbuffer_emissive: wgpu::Texture,  // Rgba16Float
    gbuffer_velocity: wgpu::Texture,  // Rg16Float
    hdr:              wgpu::Texture,  // Rgba16Float
    scene_color_copy: wgpu::Texture,  // Rgba16Float — HDR snapshot after lighting + sky (refraction)
    shadow_atlas:     wgpu::Texture,  // D32Float
    hzb:              wgpu::Texture,  // R32Float mip chain — built from final opaque depth (all backends)
    froxel_volume:    wgpu::Texture,  // Rgba16Float 3D (~160×90×64) — integrated scattering/transmittance
    froxel_history:   wgpu::Texture,  // Rgba16Float 3D — temporal accumulation

    // Ray tracing (allocated only when Capabilities::ray_query is true)
    rt:               Option<RTTargets>,
}
```

### 4. The Render Graph

Passes declare their resource dependencies; the graph resolves execution order and inserts barriers automatically. This follows the same DAG model as Godot 4.3+'s RenderingDeviceGraph and modern Vulkan/Metal engines.

```rust
struct RenderGraph {
    passes: Vec<Box<dyn RenderPass>>,
}

trait RenderPass {
    fn name(&self) -> &str;
    fn declare(&self, builder: &mut PassBuilder);
    fn prepare(&mut self, packet: &FramePacket, state: &RenderState);
    fn execute(&self, ctx: &mut PassContext);
}

impl RenderGraph {
    fn add_pass(&mut self, pass: impl RenderPass + 'static);
    fn remove_pass(&mut self, name: &str);
    fn execute(&mut self, packet: &FramePacket, state: &mut RenderState);
}
```

Games can customize the render graph — insert post-process passes, swap the volume pass implementation, add debug overlays — without touching engine internals.

#### Optional Input Slots

*(OQ 2 resolution, 2026-08-11.)* The implementation of every "public input slot" in this document — GI, per-light shadow masks, sky visibility, and any future slot: **always-bound neutral dummies + per-frame uniform flags**. Never optional bindings (WGSL has none), never pipeline permutations as architecture.

- **Declaration.** A consuming pass declares each optional slot with a name, format, and neutral value. The graph binds the producer's output when one is registered, or a 1×1 dummy holding the neutral value when none is (gi = 0, shadow mask = 1, sky visibility = 1) — so even a flag bug degrades to the fallback look, never to garbage.
- **Per-frame flags.** A uniform bitfield tells the shader which slots are live this frame. Shaders branch on it — uniform control flow, coherent across the whole dispatch, effectively free on modern GPUs. A producer that skips a frame (e.g., RT budget throttling) clears its flag with zero bind-group churn; bind groups rebuild only when producers register or unregister.
- **One pipeline per consumer.** No shader-variant system, no PSO explosion; runtime feature toggles are a flag write. Pipeline permutations remain available as a *targeted optimization* (e.g., a dedicated no-RT lighting variant for low-end hardware) — the graph knows producer presence at build time, so promoting a proven-hot variant is cheap. Trigger: profiling shows a register-pressure/occupancy win. Permutation as optimization, never as architecture.
- **Limits.** The lighting pass's full input set exceeds base WebGPU's 16 sampled textures per stage; the engine is native-only and requests elevated limits at boot (`Capabilities` reports the actuals).

### 5. Geometry Backend Convergence

Backends converge at two points.

**The shared mesh stream.** Any backend's triangle-expressible geometry — extracted meshes, imposters, proxy hulls — flows through the same retained mesh store, and therefore through the same depth, shadow, HZB, TLAS, and velocity machinery as native meshes. No per-backend integration required.

**The GBuffer.** Every registered geometry backend — built-in or game-defined — writes to the same targets. The lighting pass and everything downstream is backend-agnostic.

```
  MeshBackend ────▶ │ GBufferPass  │──┐
                    │ (rasterize)  │  │
                    └──────────────┘  │
                                      │    ┌────────────┐    ┌───────────┐
  VolumeBackend ──▶ │ VolumePass   │──┼──▶ │  GBuffer   │──▶ │ Lighting  │──▶ HDR ──▶ Post
                    │ (raymarch)   │  │    │  (shared)  │    │  (same)   │
                    └──────────────┘  │    └────────────┘    └───────────┘
                                      │
  Game backend ───▶ │ CustomPass   │──┘
  (particles,       │ (game-       │
   terrain, ...)    │  defined)    │
                    └──────────────┘
```

The Voxel Plugin's `VolumeBackend` is itself pluggable in how it renders — proxy-raster fragment raymarching (the v1 mechanism — see the GBuffer stage), mesh extraction (marching cubes / dual contouring fed into rasterization), or a hybrid where nearby volumes get full-resolution raymarching and distant volumes get extracted meshes. This is an internal detail of the backend, invisible to the rest of the pipeline — except that extracted tiers ride the shared mesh stream and therefore participate in engine passes automatically.

Translucent voxel *media* (smoke, fire) is handled separately from solid volumes: far-field media injects into the froxel grid via the public injector contract; near-field hero media renders in a plugin-owned raymarch pass in the transparency stage (see Volumetrics and Transparency). The opaque `VolumePass` never renders media.

When RT is available, the full pipeline including optional RT passes looks like:

```
  Backends ──▶ GBuffer ──▶ Shadows ──▶ Volumetrics ──┐
                   │                                  │
                   ├──▶ RT Shadows ───────────────────┤
                   ├──▶ RT GI ────────────────────────┼──▶ Lighting ──▶ Sky ──▶ Transparency ──▶ Post ──▶ Present
                   └──▶ RT Reflect ───────────────────┘
```

RT passes are optional nodes in the graph. The lighting pass consumes their outputs through its public input slots when present, and falls back to rasterized shadows / SSAO / SSR when absent (via the optional-input-slot mechanism — neutral dummies + uniform flags; WGSL itself has no optional bindings).

Plugin-provided lighting contributions — the Voxel Plugin's SVO-traced shadows and GI foremost — plug in exactly the same way: additional render graph passes that read the GBuffer and feed the lighting pass's public GI / shadow-mask slots. The existing backends and their passes are unchanged.

---

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

*(OQ 6 resolution, 2026-08-11.)* Game behavior attaches to entities through **one contract with three backends**: Rust (native, first-class), C/C++ (native, via a stable C ABI), and Lua (sandboxed gameplay-iteration tier). A prototype scripted in Lua and later ported to Rust behaves identically — the tier changes performance, never semantics.

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

| Backend | Linkage | World access | Sandbox | Threading |
|---------|---------|--------------|---------|-----------|
| Rust | Static; implements `Behavior` directly | Direct `&mut World` via context — zero overhead | No (trusted) | Serial in v1; contract permits future parallelism via declared component access |
| C/C++ | Dynamic library; stable **C ABI vtable** mirroring `Behavior` | Per-call accessor API (same surface as Lua) | No (trusted native code) | Same as Rust |
| Lua (mlua, Lua 5.4) | VM owned by the `BehaviorHost`; instances are registry-keyed tables | Per-call accessor API | Yes | Serial — a property of the single VM, not of the contract |

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

---

## Describing a Game

At its core, describing a game in smallworld is about implementing the `App` trait, populating a `World` with entities and components, and letting the engine handle the rest.

### 1. The Core Hierarchy (Entities & Components)

Every game object is an entity — an opaque ID — with components attached. Components are plain data. There is no base class and no required components; an entity with just a `Transform` and a `LightSource` is a light, an entity with a `Transform` and a `MeshRenderer` is a renderable object, and an entity with just a `GameplayTagContainer` is a logical marker.

#### Entity

```rust
// EntityId is a generational index — stable across insert/remove cycles
struct EntityId {
    index:      u32,
    generation: u32,
}

// EntityFlags control engine behavior
bitflags! {
    struct EntityFlags: u8 {
        const ACTIVE   = 0x01;  // participates in update
        const VISIBLE  = 0x02;  // participates in rendering — toggling emits draw upserts/removes through the delta stream
        const STATIC   = 0x04;  // hint: never moves (enables caching)
    }
}
```

#### Core Engine Components

These are the components the engine defines and understands. Games can define additional components.

##### Transform & Hierarchy

```rust
struct Transform {
    position: Vec3,
    rotation: Quat,
    scale:    Vec3,
}

// Computed by the engine's TransformSystem — game code reads but never writes
struct WorldTransform {
    matrix:      Mat4,
    inverse:     Mat4,
    prev_matrix: Mat4,   // previous frame, for velocity / motion vectors
}
```

Entities can form parent-child hierarchies. The engine propagates local transforms through the hierarchy to produce `WorldTransform` components automatically.

```rust
impl World {
    fn set_parent(&mut self, child: EntityId, parent: Option<EntityId>);
    fn children(&self, parent: EntityId) -> &[EntityId];
    fn parent(&self, child: EntityId) -> Option<EntityId>;
}
```

##### Fixed-Timestep Interpolation

*(OQ 6 resolution, 2026-08-11.)* Entities driven by fixed-tick simulation (physics bodies) store their previous fixed-tick transform; the extract step samples `lerp(prev_tick, curr_tick, alpha)` at the fixed-step accumulator's blend factor. Smooth motion at any refresh rate, for ≤ 1 fixed tick of visual latency. Extrapolation is rejected — it predicts through collisions and pops.

- **Two distinct "previous transforms" exist and must not be conflated.** The previous *fixed-tick* transform (interpolation input, stored per simulated entity) is not `WorldTransform.prev_matrix` (the previous *rendered frame's* matrix — the motion-vector input, derived at extract). They differ whenever render rate ≠ tick rate.
- **Only fixed-tick-driven entities interpolate.** `update()`-driven entities — notably the camera — pass through directly, so look input pays zero added latency.
- **Teleports snap.** The teleport API sets prev = curr, so a teleport never smears across a tick.

##### Camera

```rust
struct Camera {
    projection: Projection,
    target:     RenderTargetRef,   // Screen, or an offscreen texture (RTT, probes)
    priority:   i32,               // ordering among active cameras (split-screen, PiP)
    active:     bool,
    exposure:   Exposure,          // per-camera exposure (OQ 12)
}

enum Projection {
    Perspective  { fov_y: f32, near: f32, far: f32 },
    Orthographic { height: f32, near: f32, far: f32 },
}

enum RenderTargetRef { Screen, Texture(ResourceHandle<RenderTexture>) }

enum Exposure {
    Auto(AutoExposureParams),   // histogram metering — see Post-Processing
    Manual { ev: f32 },         // cinematic / artistic control
}
```

A camera is an entity with a `Transform` and a `Camera` component — view matrices derive from its `WorldTransform`. Every active camera becomes a `ViewParams` entry in the `FramePacket`; multiple active cameras give split-screen and render-to-texture without special cases.

##### Rendering

```rust
struct MeshRenderer {
    mesh:            AssetHandle<MeshAsset>,
    material:        ResourceHandle<MaterialDef>,
    cast_shadows:    bool,
    receive_shadows: bool,
    double_sided:    bool,
    lod_bias:        f32,          // multiplier on LOD distance thresholds
    render_layer:    RenderLayer,  // bitmask for camera filtering
}

struct VolumeRenderer {
    source:          VolumeSourceId,      // plain-data handle — generator registered with the Voxel Plugin
    source_params:   VolumeSourceParams,  // per-entity generator inputs (seed, offset, …) — plain data
    bounds:          AABB,
    lod_policy:      LodPolicy,
    stream_priority: StreamPriority,
}

// Generators register once with the Voxel Plugin under a stable name; entities reference them
// by handle — the same discipline as assets and behaviors. Saves reference sources by name,
// exactly as they reference assets by path (OQ 20). One generator serves any number of entities.
// (OQ 21, 2026-08-11: `source` was previously a `Box<dyn VolumeSource>` inside the component —
// moved behind a handle to restore the plain-data component rule.)
impl VoxelPlugin {
    fn register_source(&mut self, name: &str, source: impl VolumeSource + 'static) -> VolumeSourceId;
}

trait VolumeSource: Send + Sync {
    fn generate(&self, params: &VolumeSourceParams, coord: BrickCoord, world_min: Vec3) -> Option<BrickData>;
    fn bounds(&self, params: &VolumeSourceParams) -> AABB;
    fn lod_hint(&self) -> LodMeta;
}
```

##### Lighting

```rust
struct LightSource {
    kind:        LightKind,
    color:       Vec3,
    intensity:   f32,
    cast_shadow: bool,
    shadow_bias: f32,
}

enum LightKind {
    Directional { direction: Vec3, cascade_count: u8 },
    Point       { radius: f32, falloff: Falloff },
    Spot        { direction: Vec3, radius: f32, inner_angle: f32, outer_angle: f32, falloff: Falloff },
}

enum Falloff { InverseSquare, Linear }
```

##### Fog & Media

Local participating media is an entity with a `Transform` and a `FogVolume` component, injected into the froxel grid each frame. Global height fog lives in `EnvironmentParams` (OQ 11).

```rust
struct FogVolume {
    shape:      FogShape,   // Box | Sphere — local bounds derive from Transform
    density:    f32,
    albedo:     Vec3,
    emission:   Vec3,
    anisotropy: f32,        // Henyey-Greenstein g, −1..1
}
```

##### Physics

Plain-data *descriptions* — the physics provider owns simulation state internally, linked by handle and rebuilt from these on load (see the Physics section).

```rust
struct RigidBody {
    body_type: BodyType,   // Dynamic | Kinematic | Static
    mass:      f32,
    drag:      f32,
    ccd:       bool,       // continuous collision detection for fast movers
}

struct Collider {
    shape:       ColliderShape,   // Sphere | Capsule | Box | ConvexHull | TriMesh
    friction:    f32,
    restitution: f32,
    layers:      CollisionLayers, // bitmask: collision filtering
    sensor:      bool,            // trigger volume — events only, no collision response
}
```

##### Reflection Probes (spec'd, deferred)

Not v1. Trigger: **authored** interior content (buildings, ships). Probes are deliberately *not* the answer for procedural voxel interiors — a static capture goes stale on destruction; the SVO sky-visibility and voxel-traced specular slots cover those. Captured via aux views, prefiltered by the Environment/IBL machinery, assigned per cluster like lights, sampled with parallax box projection.

```rust
struct ReflectionProbe {
    shape:          ProbeShape,   // Box | Sphere — extents from Transform
    blend_distance: f32,
    resolution:     u32,          // cubemap face size
    update:         ProbeUpdate,  // Static (capture once) | Dynamic { interval_frames: u32 }
}
```

##### Materials

Shared via `ResourceHandle` — multiple entities can reference the same material. Mutable at runtime.

```rust
struct MaterialDef {
    base_color:             Vec4,
    roughness:              f32,
    metallic:               f32,
    emissive:               Vec3,
    emissive_intensity:     f32,
    albedo_map:             Option<AssetHandle<TextureAsset>>,
    normal_map:             Option<AssetHandle<TextureAsset>>,
    roughness_metallic_map: Option<AssetHandle<TextureAsset>>,
    emissive_map:           Option<AssetHandle<TextureAsset>>,
    alpha_mode:             AlphaMode,
    double_sided:           bool,
}

enum AlphaMode { Opaque, Mask(f32), Blend }
```

#### Custom Game Components

Games define their own components as plain Rust structs. The only requirement is `Send + Sync + 'static`.

```rust
// Game-defined component — the engine doesn't know about it
struct Health {
    current: f32,
    max:     f32,
}

struct Inventory {
    slots: Vec<Option<ItemId>>,
    capacity: usize,
}

// Attach to entities just like engine components
world.add(player, Health { current: 100.0, max: 100.0 });
world.add(player, Inventory { slots: vec![None; 20], capacity: 20 });
```

### 2. The Gameplay Framework

Unlike UE5, smallworld does not impose a rigid GameMode/GameState/Controller hierarchy. Instead, it provides building blocks that games compose as needed.

#### The App Trait

The game's entry point. The engine calls these methods at defined points in the frame.

```rust
trait App {
    fn init(&mut self, ctx: &mut GameContext);
    fn update(&mut self, ctx: &mut GameContext, dt: f32);
    fn fixed_update(&mut self, ctx: &mut GameContext, fixed_dt: f32);
    fn shutdown(&mut self);
}
```

- `init` — called once after the engine initializes the World. Load assets, spawn initial entities, set up game state.
- `update` — called once per frame with variable delta time. Process input, run gameplay logic, animate.
- `fixed_update` — called at a fixed rate (default 60 Hz, configurable). Physics integration, network tick, anything that needs deterministic timestep. May run 0–N times per frame depending on accumulated time. **Engine guarantee (OQ 19):** no engine system introduces nondeterminism into fixed-tick simulation — this keeps lockstep/rollback netcode viable when networking arrives.
- `shutdown` — called once before exit. Save state, clean up.

#### GameContext

Everything the game needs to interact with the engine, bundled into a single borrow.

```rust
struct GameContext<'a> {
    world:  &'a mut World,
    input:  &'a Input,
    time:   &'a Time,
    assets: &'a mut AssetServer,
    audio:  &'a mut AudioCommands,
    events: &'a mut EventBus,
    window: &'a WindowState,
}

// Registration APIs and render feedback are methods on GameContext,
// backed by engine state outside the public fields:
impl GameContext<'_> {
    // Init-time registration
    fn register_geometry_backend(&mut self, extractor: impl GeometryExtractor + 'static,
                                 renderer: impl GeometryRenderer + 'static);
    fn register_system(&mut self, phase: Phase, system: impl System + 'static);
    fn set_draw_processor(&mut self, pass: &str, processor: impl DrawProcessor + 'static);

    // Render feedback — fn feedback(), fn gpu_frame_time(): defined in
    // Frame Pipeline — Render-to-Game Feedback (not repeated here).
}

struct Time {
    dt:       f32,    // variable delta (seconds)
    elapsed:  f64,    // total seconds since start
    frame:    u64,    // frame counter
    fixed_dt: f32,    // fixed timestep (e.g. 1/60)
}

struct WindowState {
    width:        u32,
    height:       u32,
    scale_factor: f64,
    focused:      bool,
    mode:         WindowMode,
}
```

#### Engine Entry Point

```rust
struct EngineConfig {
    title:           String,
    window_mode:     WindowMode,
    fixed_timestep:  f32,        // default 1/60
    pipeline_mode:   PipelineMode,
    pacing:          PacingConfig,   // vsync, target frame time, DRS, latency mode (OQ 8)
    render_budget:   RenderBudget,
    log_level:       LogLevel,
}

impl Engine {
    fn run(config: EngineConfig, app: impl App + 'static) -> !;
}
```

The engine creates the World internally and hands it to `App::init()` via `GameContext`. This ensures internal component stores and change tracking are configured before the game touches anything.

#### Systems

Games can register systems — functions that run each frame over component data — for logic that doesn't belong in the monolithic `App::update()`.

```rust
trait System: Send {
    fn name(&self) -> &str;
    fn run(&mut self, world: &mut World, dt: f32);
}

impl GameContext<'_> {
    fn register_system(&mut self, phase: Phase, system: impl System + 'static);
}

enum Phase {
    PreUpdate,     // before App::update — engine systems (input, time)
    Update,        // during App::update — game systems
    PostUpdate,    // after App::update — physics, animation
    LateUpdate,    // after PostUpdate — hierarchy propagation, bounds recomputation
}
```

Engine-internal systems (transform propagation, streaming demand, change tracking) run in `LateUpdate` and are not user-visible.

### 3. World Building

#### Scenes & Levels

A `World` contains all entities for the current level. Level transitions swap the entire World. For streaming open worlds, the engine supports region-based loading — entities within a geographic region are spawned and despawned based on camera distance (see the Streaming section for the full two-layer design).

```rust
struct LoadedScene {
    meshes:    Vec<(String, MeshAsset)>,
    materials: Vec<(String, MaterialDef)>,
    textures:  Vec<(String, TextureAsset)>,
    instances: Vec<SceneInstance>,
    lights:    Vec<SceneLight>,
}

struct SceneInstance {
    name:      String,
    mesh:      usize,       // index into meshes
    material:  usize,       // index into materials
    transform: Transform,
}

impl LoadedScene {
    fn spawn(&self, world: &mut World);
}
```

Compound assets (glTF, custom scene format) are loaded through the `AssetServer` and produce `LoadedScene` values that bulk-insert entities. `LoadedScene` is an *import-time* product (DCC interchange); authored cells and saves use the OQ 20 document format — the two meet at spawn time, not on disk.

#### Entity Hierarchy

Entities can form parent-child trees. A character entity might parent its weapon, particle emitters, and audio sources. When the parent moves, children inherit the transform. When the parent is despawned, children are despawned recursively.

This is equivalent to UE5's `SetupAttachment` component tree — but flattened into a simple parent ID on the entity rather than a component hierarchy.

#### Serialization & Save Games

*(OQ 20 resolution, 2026-08-11 — shape committed now, implemented when save games are first needed.)* Persistence follows the opt-in registry model — the same shape as Godot 4's exported properties + `PackedScene` and Bevy's `DynamicScene`:

- **Opt-in component registry.** Components register for persistence under a **stable name + version**, with serde-derived (de)serialization and per-version migration hooks. The registry is shared infrastructure, not save-specific: replication (OQ 19) consumes the same component identity + codecs, and a future reflection layer (editor inspectors) backs the same registry without changing the save format. One registry, three consumers.
- **Save documents.** Header (engine + save versions) + entity section (registered components over a chosen entity set) + game-defined sections. Format-agnostic via serde — RON in dev (diffable), binary + compression shipping.
- **Loading spawns fresh entities.** `EntityId`s are never stable across sessions. Component fields that reference entities use the **`EntityRef` wrapper type**, so the loader knows every reference site and remaps automatically — the dangling-reference bug class is eliminated structurally, not documented around. Asset handles serialize as paths/UUIDs and re-resolve through the `AssetServer`.
- **Transient state is rebuilt, never saved.** Transforms re-propagate, GPU resources re-upload, behaviors re-`init`. **Discipline rule (load-bearing):** persistent state lives in components; behaviors and VMs hold only transient state — which is why Lua state never needs serializing. A dev-mode audit warns when a save touches unregistered component types, so opt-in silence can't bite silently.
- **Bulk world data is out of scope.** Voxel regions live in streaming-owned region files (OQ 17); a save *references* region state, never inlines it. Saves stay small; worlds stay on disk.

### 4. Assets & Resources

#### AssetServer

Assets are loaded asynchronously and accessed via generation-counted handles.

```rust
struct AssetServer {
    registry: HashMap<AssetId, AssetEntry>,
    loaders:  Vec<Box<dyn AssetLoader>>,
    io_pool:  ThreadPool,
    watcher:  Option<FileWatcher>,  // hot-reload in dev builds
}

impl AssetServer {
    fn load<T: Asset>(&mut self, path: &str) -> AssetHandle<T>;
    fn state<T: Asset>(&self, handle: AssetHandle<T>) -> AssetState;
    fn get<T: Asset>(&self, handle: AssetHandle<T>) -> Option<&T>;
    fn unload<T: Asset>(&mut self, handle: AssetHandle<T>);
}

trait AssetLoader: Send + Sync {
    fn extensions(&self) -> &[&str];
    fn load(&self, bytes: &[u8], path: &Path) -> Result<Box<dyn Asset>>;
}

enum AssetState { Unloaded, Loading, Loaded, Failed(String) }
```

Games register custom asset loaders for game-specific formats. The engine provides built-in loaders for meshes (glTF/GLB), textures (PNG, KTX2), audio (WAV, OGG), and scenes.

GPU-destined bulk data is decoded directly into staging-pool regions (see Staging Pool & Upload Path); the `AssetServer` retains CPU-side copies only for assets that need CPU access, so GPU-only assets cost no long-lived CPU memory.

#### Handles

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

- `AssetHandle<T>` — references an immutable, shared asset (mesh geometry, texture pixels, audio clip). Many entities can hold the same handle.
- `ResourceHandle<T>` — references a mutable resource in the World (materials). The game can modify these at runtime (e.g., animate a material's emissive intensity).

Both use generational indices for use-after-free detection.

#### Asset Types

```rust
struct MeshAsset {
    vertices: Vec<Vertex>,
    indices:  Vec<u32>,
    bounds:   AABB,
    lods:     Vec<MeshLod>,
}

struct TextureAsset {
    pixels: Vec<u8>,
    width:  u32,
    height: u32,
    format: TextureFormat,
    mips:   bool,
}

struct Vertex {
    position: [f32; 3],
    normal:   [f32; 3],
    uv:       [f32; 2],
    tangent:  [f32; 4],  // xyz + bitangent sign
}
```

### 5. Input

Accumulated per-frame on the main thread. Provides held/pressed/released semantics for digital inputs and continuous values for analog inputs.

```rust
struct Input {
    keyboard:    KeyboardState,
    mouse:       MouseState,
    controllers: [Option<ControllerState>; 4],
}

impl Input {
    fn key_held(&self, key: KeyCode) -> bool;
    fn key_pressed(&self, key: KeyCode) -> bool;      // true on the frame the key goes down
    fn key_released(&self, key: KeyCode) -> bool;     // true on the frame the key goes up
    fn mouse_position(&self) -> Vec2;
    fn mouse_delta(&self) -> Vec2;
    fn mouse_button_held(&self, button: MouseButton) -> bool;
    fn scroll_delta(&self) -> f32;
    fn controller(&self, index: usize) -> Option<&ControllerState>;
}

struct ControllerState {
    left_stick:  Vec2,
    right_stick: Vec2,
    left_trigger:  f32,
    right_trigger: f32,
    buttons: ControllerButtons,
}
```

### 6. Audio

Game code issues audio commands; the audio system runs on a dedicated thread. No direct API access from game code — same server pattern as the rendering thread.

```rust
struct AudioCommands {
    commands: Vec<AudioCommand>,
}

impl AudioCommands {
    fn play(&mut self, clip: AssetHandle<AudioClip>, params: PlayParams) -> SoundHandle;
    fn stop(&mut self, handle: SoundHandle);
    fn set_listener(&mut self, position: Vec3, forward: Vec3, up: Vec3);
    fn set_volume(&mut self, handle: SoundHandle, volume: f32);
    fn set_position(&mut self, handle: SoundHandle, position: Vec3);
}

struct PlayParams {
    volume:   f32,
    pitch:    f32,
    spatial:  bool,
    position: Vec3,
    looping:  bool,
}
```

### 7. Events

A typed event bus for decoupled communication between game systems. The bus is **double-buffered** *(OQ 6, 2026-08-11)*: events sent during frame N become readable during frame N+1 and are dropped at its end. `read<E>()` always returns the previous frame's events, so results are deterministic regardless of system ordering — no send-before-read hazards within a frame.

```rust
struct EventBus {
    channels: HashMap<TypeId, Box<dyn Any>>,
}

impl EventBus {
    fn send<E: Event>(&mut self, event: E);
    fn read<E: Event>(&self) -> &[E];
}
```

### 8. Change Tracking

The engine tracks which entities and components have been modified each frame. This drives the extract step — only dirty data is re-extracted for the Render Thread.

```rust
struct ChangeTracker {
    spawned:         HashSet<EntityId>,
    despawned:       HashSet<EntityId>,
    dirty:           HashMap<TypeId, HashSet<EntityId>>,  // per-component-type dirty sets
    dirty_resources: HashSet<ResourceId>,  // mutable resources (materials) — drives UpdateMaterial ops
}

impl ChangeTracker {
    fn is_dirty<C: Component>(&self, entity: EntityId) -> bool;
    fn dirty_set<C: Component>(&self) -> &HashSet<EntityId>;
    fn spawned(&self) -> &HashSet<EntityId>;
    fn despawned(&self) -> &HashSet<EntityId>;
}
```

Mutations through `World::get_mut<C>()` automatically mark the component dirty; mutations through a `ResourceHandle` (e.g., animating a material) mark the resource dirty. The change tracker is cleared after the extract step completes.

---

## Streaming

*(OQ 17 resolution, 2026-08-11.)* Streaming is **two layers with different owners**, coordinated by demand signals and a single budget arbiter. Which *entities exist* is Game Thread World mutation; which *data is resident* is streaming-side truth (OQ 21). Conflating the two breaks the thread-ownership rules; splitting them is UE5's own shape (World Partition ∥ streaming pools).

### Layer 1 — World Streaming (entities)

The World Partition analog. Space divides into **uniform grid cells** — grids plural: independent grids per content class (e.g., 256 m gameplay entities, 1 km landmarks), each with its own cell size and load range; 2D partitioning by default, 3D as a config option. Uniformity is the point: O(1) cell lookup, stable cell identity (stable file names — what save references need), and predictable load sets (a source moving at speed *v* crosses a computable number of cell boundaries per second, so worst-case IO is budgetable).

- **Entities auto-assign to cells by bounds**, with an `ALWAYS_LOADED` override for global entities.
- **`StreamingSource` drives loading.** Players/cameras carry one; cells within range load (entities batch-spawn — per the existing load-time-spawn rule), cells out of range unload (batch-despawn). Range rings with hysteresis prevent boundary thrash.

```rust
struct StreamingSource {
    range:    f32,   // load radius
    priority: u8,    // arbiter class for demand originating from this source
}
```

- **Cell content = OQ 20 documents: base + overlay.** The base is authored content or a generation cache; the overlay holds persistent runtime edits (destruction). Load = base ∪ overlay. Same serde document format as scenes and saves — one format for authored content, generated caches, and persistence. Cell files are named by grid coordinates: stable across sessions.
- **HLOD proxies:** the contract is reserved (a cell may carry a far-proxy entity set); generation tooling is deferred.

### Layer 2 — Detail Streaming (data residency)

Which data for existing entities is resident, at what quality. **v1 client: voxel bricks.** Texture mip streaming and mesh LOD streaming are later clients of the *same* manager through the same thin client interface — designed for now, not retrofitted.

#### The Demand/Fulfill Pipeline

Every arrow is a channel; every stage uses machinery already specified:

```
Game LATE phase ── demand: (coord, wanted LOD, priority) ──▶ Streaming Coordinator
Coordinator ── dispatch ──▶ io_pool: region-file read  |  worker pool: VolumeSource::generate
tasks ── decode/write directly into ──▶ staging-pool regions (OQ 5)
Coordinator ── UploadBatch ──▶ Render Thread PREPARE: record GPU copies, publish residency
Render Thread ── FrameFeedback advisories (fulfilled / evicted / culled) ──▶ demand planner
```

- **The Streaming Coordinator is a dedicated low-priority thread** — it owns the priority queue and budget arbiter exclusively (thread-ownership applied, not excepted), and it is a *dispatcher, never a worker*: IO and decode run on the existing pools. Event-driven, parked when idle; completion-to-dispatch latency is microseconds, keeping the four-stage pipeline full instead of bubbling a frame per stage.
- **Cancellation** is generation-stamped queue entries — when the camera turns, stale demand dies in the queue, not in flight.
- **Residency publishes render-side** (brick pool tables) at copy-record time; the game thread only ever learns residency through advisory feedback, and never asserts it (OQ 21).

#### The Residency Invariant

**The coarse tier of everything is pinned resident** — SVO root/coarse bricks, lowest mips, far-tier extracted meshes. Every possible residency miss therefore has a rendering answer (fall back through coarser parents); the failure mode under any pressure is *blur, never holes, never stalls*. This is the virtual-texturing lesson applied engine-wide, and it is what makes every budget decision below safe to make.

Eviction above the pinned tier: **priority classes** (pinned → active-view → shadow/aux-view → prefetch), **LRU within class**, hysteresis (freshly-uploaded and recently-requested entries are evict-protected for a cooldown).

**LOD transitions gate on residency (OQ 10).** A fade-in never starts until the target LOD is resident; the demand rings *anticipate* transitions by requesting the next LOD one band before its transition distance. Fade-*down* is always possible unconditionally, courtesy of the pinned coarse tier. Transitions therefore never wait on IO and never pop because of it.

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
- **Contract rule:** `VolumeSource::generate` must be *pure* with respect to `(params, coord)` — required for cache coherence, and it keeps the fixed-tick determinism story (OQ 19) open for generated worlds.
- **Edits are never cache.** Persistent modifications always live in the cell overlay, regardless of policy — the cache can be deleted wholesale at any time without losing player-visible state.

---

## Physics

*(OQ 16 resolution, 2026-08-11.)*

### Provider Model

Physics is a **provider behind one engine-shaped interface** — the same replaceable-node philosophy as the temporal resolve and the behavior backends. v1 provider: **rapier** (pure Rust, zero FFI, deterministic mode). Named up/side-grade candidates: **Jolt** (quality and scale headroom; C++ FFI) and PhysX. Swapping providers is a port of one module, never a rip-and-replace — because the interface obeys three rules that keep the swap real:

1. **Shaped by engine consumption, never by provider wrapping.** Descriptions in (`RigidBody`/`Collider` components), transforms + events + query answers out. The interface covers what the engine *uses*, not the union of provider features.
2. **No lowest-common-denominator bloat.** Provider-specific capability goes through a typed extension escape hatch (`provider.extension::<JoltExt>()`) — games use it knowingly, at their own portability cost; the core interface never grows to accommodate one provider.
3. **Determinism is part of the contract.** A provider must offer a deterministic mode to be certified for fixed-tick use (the OQ 19 guarantee).

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

    // Queries — the game-thread read API
    fn raycast(&self, ray: Ray, filter: QueryFilter) -> Option<RayHit>;
    fn sweep(&self, shape: &Shape, motion: &Motion, filter: QueryFilter) -> Option<SweepHit>;
    fn overlap(&self, shape: &Shape, filter: QueryFilter) -> Vec<EntityId>;
}
```

### Integration

- **Physics steps exclusively in `fixed_update`** — determinism and solver stability both demand it. This closes the other half of OQ 6's interpolation contract: physics writes fixed-tick transforms; extract interpolates them.
- **The physics world is a side structure.** `RigidBody` and `Collider` are plain-data *descriptions*; the provider owns simulation state internally, linked by handle, created/destroyed from the change tracker's spawn/dirty sets. Components stay serializable (OQ 20) — simulation state rebuilds from descriptions on load.
- **Sync-back** after each fixed step goes through the normal `get_mut` path (change tracking fires naturally), storing prev-tick state for interpolation. `PhysicsEvent`s enter the double-buffered event bus.
- **Queries** are the game-thread read API — OQ 22's gameplay raycasts ride this for collider-bearing entities.

### Worker-Pool Split

**v1: two pools** — a render pool and a game pool, sized by core count (configurable). Culling never waits on physics: with no preemption in any Rust task system, isolation is the only *guaranteed* fix for priority inversion. The classic utilization objection evaporates under our own architecture — the 2-frame pipeline runs game frame N+1 and render frame N concurrently, so both pools stay busy in steady state. The physics provider's internal parallelism binds to the game pool.

**v2: a task-graph scheduler** — declared dependencies + priorities (the UE Task Graph analog), built on crossbeam's work-stealing primitives (`crossbeam-deque`, the same foundation rayon stands on). One future scheduler serves parallel systems (OQ 6), physics, and streaming decode alike.

---

## Lifecycle

*(OQ 15 resolution, 2026-08-11.)* Engine-level lifecycle: surface events, device loss, and shutdown.

### The Control Channel

Lifecycle events ride an **out-of-band control channel** (control/data-plane separation, per Gregory), main thread → Render Thread: `Resized`, `ScaleFactorChanged`, `Minimized/Restored`, `DeviceLost`, `Quit`. Control must be deliverable independent of packet flow — a paused game still resizes; a stalled pipeline still quits. **Transport is out-of-band; application is frame-synchronized:** the Render Thread drains the control channel at the top of RECEIVE and applies changes only between frames. Packets stamp the display size they were built against; a packet built pre-resize presents with scaling for that one transient frame. The data plane (`FramePacket`) never carries control.

### Resize

Main thread receives the window event, updates `WindowState` (game-visible), and sends `Resized` on the control channel. At the next frame boundary the Render Thread reconfigures the surface and reallocates display-resolution targets (TAAU history is display-res and resets on resize; internal-res maxima follow the new display size). `SurfaceError::Outdated`/`Lost` at acquire → reconfigure and retry once, else skip present that frame. There is no crash path through resize.

### Device Loss

**Invariant (architectural law): GPU memory is always a cache — no authoritative state lives only on the GPU.** This is already true by construction (retained `RenderScene` is CPU-side; bricks refill from region files/generators; assets re-load through the normal path; clipmap/froxels/histories/TLAS are transient and rebuild), and it is what keeps full recovery permanently possible.

- **v1: fatal with grace.** `DeviceLost` on the control channel → drain what is drainable, fire the game's save hook, emit diagnostics, exit through the teardown protocol.
- **Scheduled hardening: the recovery walk.** Pause the loop → recreate the device → recreate pools → re-request contents through the existing asset/streaming paths → transients rebuild over the next frames → resume. Additive, thanks to the invariant; deferred because device loss is rare and the test burden is the real cost.

### Teardown Protocol

Explicit staged shutdown — never `Drop`-order across five threads. Channel closure is the universal backstop signal; every stage has a deadline (~2 s) after which it is logged and forced. The process never hangs on exit.

1. **Stop simulation.** Exit the main loop; run `App::shutdown` and behavior `shutdown` callbacks **while all services still live** — World, AssetServer, streaming, IO — so saves flush through normal paths.
2. **Quiesce producers.** The streaming coordinator rejects new demand, mass-cancels its queue (generation stamps), **flushes pending region-file writes**, then dissolves.
3. **Drain the pipeline.** Close the packet and control channels; the Render Thread finishes in-flight frames and exits; GPU wait-idle; staging-pool and readback-ring fences complete; pools release.
4. **Stop services.** Audio drains and stops; worker pools join.
5. **Destroy.** GPU resources → device → window → process exit (`Engine::run() -> !` holds).

The ordering that bites: stage 1 before stage 2 — shutdown callbacks that save must run while the IO machinery is alive, not during destruction.

---

## Profiling & Instrumentation

*(OQ 24 resolution, 2026-08-11.)* One instrumentation API, multiple sinks — the shape UE (`stat` + Unreal Insights) and Unity (`ProfilerMarker` + Profiler) both converged on, with **Tracy playing the deep-timeline role** so we never build one. Engine/game split: the engine owns the markers API, collection across all execution contexts, and the sinks; games annotate their own systems and behaviors through the *same* macros and read the same overlay.

### The Instrumentation API (engine primitive)

```rust
profile_scope!("cull.shadow_views");    // scoped hierarchical timer, any thread
counter!(DRAW_CALLS, n);                // per-frame counter
gauge!(STREAMING_QUEUE_DEPTH, depth);   // sampled value
```

- Every execution context registers a **named thread lane**: Game, Render, game-pool workers, render-pool workers, Streaming Coordinator, Audio, IO pool.
- **Zero-cost rule.** All macros compile to no-ops in shipping builds; in dev builds, overhead is nanoseconds per scope with no client attached.
- Games use the identical macros — a game system's scopes appear in the same lanes and overlay as engine scopes.

### Sinks

| Sink | Role | When |
|------|------|------|
| **Tracy** | Deep dives: zones, lock contention, memory (alloc hook), GPU lanes, capture files | Dev machine, on attach |
| **egui overlay** | Always available: frame graph, per-thread ms, top-N scopes, counters, **budget table** | Any dev build, toggle key |
| **chrome-trace JSON export** | CI captures, bug reports, offline diffing | On demand |

- **GPU timeline unification.** `GpuTimingFeedback`'s per-pass timings — already frame-stamped by the readback ring — feed Tracy's GPU context, so CPU and GPU lanes sit on one timeline.
- Shipping telemetry (player-machine aggregation) is out of scope for v1; the trace export is the hook it would later build on.

### The BudgetRegistry — receipts for every budget

Every named budget in this document (GPU pools, staging pool, deform output, GI clipmap, froxels, streaming IO/upload/decode) **registers a gauge at creation**: budget, current usage, peak. The overlay's budget table renders whatever is registered — adding a budget without receipts is structurally impossible. Budget-consuming systems (the DRS controller, the streaming arbiter, deform LOD capping) read the same gauges they publish.

### The Standard Counter Set

Spec'd so tooling can rely on them: draw calls and instances per view; culled counts per view (mirroring the `OcclusionFeedback` advisory — that remains the game-facing data path); brick residency (resident / pinned / evicted this frame); streaming queue depth and in-flight bytes; staging in-flight regions; TLAS instance count; deformed instance count. Games add their own counters through the same macro.

---

## Frame Lifecycle

The complete sequence of a single frame, showing which thread owns each phase.

### Game Thread

| Phase | Action |
|-------|--------|
| **INPUT** | Main thread accumulates window events into `Input` snapshot |
| **FIXED** | `App::fixed_update()` runs 0–N times at fixed timestep |
| **UPDATE** | `App::update()` runs once. Game systems mutate World |
| **LATE** | Engine systems: hierarchy propagation, bounds recomputation, streaming demand |
| **EXTRACT** | Diff `&World` via `&ChangeTracker` into a `FramePacket` (views, lights, deltas, resource ops), interpolating fixed-tick transforms at the accumulator alpha; send through channel |
| **CLEAR** | Clear change tracker. Game thread free for next frame |

### Render Thread

| Phase | Action |
|-------|--------|
| **RECEIVE** | Drain control channel (resize/device events — frame-boundary application); block on packet channel, receive `FramePacket` |
| **PREPARE** | Apply `ResourceOp`s and scene deltas — upload resources, update the retained `RenderScene` |
| **CULL** | Derive shadow views; collect TLAS instances (pre-cull); per-view frustum + occlusion culling; produce sorted, batched draw lists |
| **RECORD** | Render graph executes: each pass records GPU commands |
| **SUBMIT** | Throttle on GPU queue depth (`max_gpu_frames_in_flight` — OQ 8), then `queue.submit()` |
| **PRESENT** | Swapchain present. Send `FrameFeedback`. Loop back to RECEIVE |

### Ownership Boundaries

| Data | Owner | Crosses boundary as |
|------|-------|---------------------|
| World, Components | Game Thread | Read-only in extract |
| FramePacket (deltas + views) | Produced by Game, consumed by Render | Owned value through channel (Game → Render) |
| RenderScene (retained draw data) | Render Thread | Never — updated only by applying packet deltas |
| FrameFeedback | Produced by Render, consumed by Game | Owned value through channel (Render → Game, ~2-frame lag; GPU data via readback ring) |
| Device-local GPU resources + submission | Render Thread | Never — game code uses handles |
| Staging buffers (CPU-visible, mapped) | Engine staging pool (thread-safe) | `StagingRef` through `ResourceOp`; fence-reclaimed after the GPU copy |
| Asset bulk data | AssetServer + staging pool | Decoded directly into mapped staging off-thread; never copied by value |
| Input | Main Thread | Snapshot borrowed by game tick |
| Audio commands | Collected on Game Thread | Drained by audio server each frame |

---

## Open Questions

Round 1 (OQ 1–23): the decision backlog from the 2026-08-11 review round — **fully resolved as
of 2026-08-11**. Entries are kept as decision records: each captures the choice, the rationale,
the rejected alternatives, and where the spec lives. Deferred items *inside* resolutions (v2
tiers, capability-gated upgrades, scheduled hardening) carry their own explicit adoption
triggers and need no separate tracking here.

Round 2 (OQ 24–28): opened 2026-08-11 from the Gregory (*Game Engine Architecture*) subsystem
audit — the chapters the doc only covered where they intersected the render pipeline. Framing
rule for all five: **decide the engine primitives and traits; games compose them** — Gregory
intermixes game and engine concerns, we deliberately do not.

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
   world-radiance representation, maintained whenever software GI *or* RT is active; Lumen's
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
   *immutable* data; Principle 3 constrains game code, and the real invariant is Render-Thread
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
    trigger: *authored* interiors. Specs: Environment Capture & IBL (Sky stage), Lighting Pass
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
18. **[RESOLVED 2026-08-11] UI.** Two-track stance: **dev/debug tooling = egui**, integrated
    now as a final render-graph pass over the post-processed image; the **game-facing UI
    framework** (retained widgets, layout, theming) is a committed post-v1 subsystem that gets
    its own design round — not rushed, and not blocking engine v1.
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
    general *engine-internals* mechanism (answer: no — side structs and dense pools win there);
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
25. **Animation runtime.** The pose-computation half feeding the DeformPass: skeleton + clip
    assets, an animator component, blending architecture (trees, layers, masks, additive),
    state machines, animation events, root motion, clip compression, bone sockets/attachment.
    Engine provides sampling/blending primitives and the palette contract; games compose
    graphs and logic.
26. **Audio engine.** Behind the `AudioCommands` surface: the build-vs-middleware decision,
    mixer/bus hierarchy, DSP effects (reverb, filters, occlusion), voice management and
    virtualization, streaming audio (music), the spatialization model. Currently the doc
    specifies an API and a thread, not an audio engine.
27. **Resource pipeline & filesystem.** The offline half of assets: cooking/conditioning
    pipeline (import → engine-native formats), filesystem abstraction (mounts, archives/paks,
    platform paths), asset identity (GUID vs. path), inter-asset dependency management,
    refcount/unload policy. The runtime half (AssetServer, staging, streaming) is done.
28. **Physics contract completion.** Joints/constraints (component + `PhysicsProvider` API —
    doors, ropes, ragdolls all sit on them) and the character controller (the most
    gameplay-facing physics feature, currently unspecified). Ragdolls/vehicles come later but
    depend on joints.
