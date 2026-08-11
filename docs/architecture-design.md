# Smallworld Engine Architecture

## Overview

Smallworld is a hybrid game engine built from scratch in Rust on top of wgpu. It is a complete game engine — not a renderer, not a voxel engine. Voxel volumes and triangle meshes are both first-class geometry primitives sharing the same lighting model. The engine supports rasterization and raytracing simultaneously, with rasterization as the primary path and raytracing reserved for effects like shadows and global illumination.

The architecture takes the best ideas from Unreal Engine 5 — the Game Thread / Render Thread split, the Scene Proxy extraction model, composition-over-inheritance, data-driven design — and rebuilds them in idiomatic Rust without the C++ baggage. No `UObject` reflection system, no garbage collector, no `UPROPERTY` macros. Instead: trait objects for polymorphism, channels for cross-thread communication, SlotMap arenas for stable handles, and Rust's ownership system as the thread-safety guarantee.

### Design Principles

1. **Data-driven.** Components are plain data structs. Systems are functions that operate on component stores. No inheritance hierarchies, no virtual dispatch on hot paths.
2. **Thread ownership.** Each thread owns its data exclusively. Communication between threads happens via owned value-typed packets sent through channels — never shared mutable state.
3. **Game–render firewall.** Game code never sees a `wgpu::Device`, a bind group, or a GPU buffer. The extract step is the boundary. Everything above it speaks in transforms, materials, and handles. Everything below it speaks in draw commands and GPU resources.
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

Three execution contexts, each with clear ownership boundaries:

| Context | Owns | Communicates via |
|---------|------|------------------|
| **Game Thread** (main) | World, Input, Time, Systems, AssetServer | Sends `FramePacket` to Render Thread; receives `FrameFeedback` (N-2) |
| **Render Thread** (dedicated) | GpuContext, RenderScene (retained draw data), RenderGraph, GPU resource pools, render targets | Receives `FramePacket`; sends `FrameFeedback` back |
| **Worker Pool** (rayon, work-stealing) | Nothing persistent — borrows work items | Scoped tasks with `join()` / `parallel_for()` |

The Worker Pool is shared between both threads. The Game Thread uses it for physics broadphase, animation sampling, and streaming demand computation. The Render Thread uses it for frustum culling, draw call sorting, and batch generation.

### Render-to-Game Feedback

The pipeline is not one-directional. The Render Thread sends a `FrameFeedback` back to the Game Thread after each frame is submitted. This travels through a separate channel — the Game Thread typically reads feedback from frame N-2 while processing frame N. Never a synchronous wait.

Feedback data has two ages. CPU-side data (cull statistics) describes the frame the feedback was sent after. GPU-derived data (timestamps, occlusion queries, compute readbacks) is older: at submit time the GPU has not yet executed the frame, so query results are collected through a **frames-in-flight readback ring** — 2–3 buffered query/readback sets, polled via `map_async` without blocking — and each GPU-derived datum is stamped with the frame it actually measures. The Render Thread never blocks on the GPU to assemble feedback.

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
    OcclusionQuery { source_frame: u64, entity: EntityId, visible_samples: u32 },
    ComputeResult  { source_frame: u64, tag: u32, data: Vec<u8> },
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

---

## The Render Thread

The Render Thread receives a `FramePacket` each frame and translates it into GPU work through a structured sequence of passes organized by a render graph.

### 1. Receive & Prepare

The Render Thread blocks on the channel until a `FramePacket` arrives. It then processes any `ResourceOp` entries — uploading new meshes and textures to GPU pools, updating material uniform data, and freeing resources for despawned entities — and applies the packet's scene deltas to the retained `RenderScene`: upserting and removing commands in the shared mesh draw store, and handing each backend's custom payload to its renderer half.

This is the only point where the Render Thread mutates its persistent state (GPU resource caches and the `RenderScene`). Everything after this is read-only traversal of the scene and packet.

### 2. Visibility & Culling

Before anything is drawn, the engine determines what each view can see. Culling is **per-view**: the packet's main and auxiliary views, plus shadow views the Render Thread derives from the shadow-casting lights (cascade fitting against the main view — the equivalent of UE5's InitViews shadow setup). Each registered geometry renderer culls the retained scene per view via `renderer.cull()`, allowing geometry types to implement specialized strategies (e.g., octree traversal for volumes vs. flat AABB tests for meshes).

- **View setup.** Collect this frame's views: main camera, aux views (render-to-texture, probes), and one view per shadow cascade / shadowed local light.
- **RT instance collection.** When RT is enabled, the TLAS instance list is gathered **before any per-view culling**, from the retained scene under its own, larger culling domain (an RT radius around the camera — off-screen geometry must still exist for shadows, reflections, and GI). See the Ray Tracing section.
- **Frustum culling.** Test every draw command's world-space AABB against each view's frustum planes. Parallelized across the worker pool.
- **Occlusion culling (HZB).** Using the Hierarchical Z-Buffer built from the previous frame's *final opaque depth* — all backends, not just meshes (see the GBuffer stage) — test remaining objects in the main view to discard those hidden behind large occluders.
- **LOD selection.** For volumes and meshes with LOD levels, select the appropriate detail tier based on screen-space size or distance. Each backend owns its LOD strategy.
- **Sort & batch.** Per view: opaque draws sorted front-to-back (minimize overdraw), transparent draws back-to-front. Draws sharing pipeline state are merged into instanced batches.

### 3. Depth Pre-Pass

Establishes the scene's depth early to prevent overdraw in the full GBuffer pass.

- **Early Z.** Opaque meshes render depth-only to the Z-buffer.

The HZB is deliberately *not* built here — a pre-pass HZB would contain only mesh depth, and geometry from other backends (raymarched volumes, custom plugins) would never act as occluders. It is built after the GBuffer stage, once every backend has contributed depth.

### 4. GBuffer Pass (Geometry)

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

**Capability-gated upgrade tier (deferred to the GPU-driven work, OQ 7).** A compute visibility-buffer variant — depth+payload packed via 64-bit atomic min/max (`Capabilities::int64_atomic_min_max`; on Metal this requires Apple M2-class "Nanite atomics") — is the sanctioned future optimization, not a rejected option. It is deferred because it needs its own coverage/binning and resolve machinery, the fragment path must exist for baseline hardware anyway, and its win is unproven for large-box proxies (Nanite's software raster exists to beat micro-triangle raster inefficiency, which volume proxies don't have). **Adoption trigger:** profiling shows the fragment path limiting on capable hardware, or variable-rate marching is needed.

#### Picking & Debug IDs

*(OQ 22 resolution, 2026-08-11.)* There is deliberately **no per-frame ID target** in the GBuffer — per-pixel IDs would pay ~4 bytes/pixel of write+read bandwidth every frame for consumers that are better served elsewhere:

- **Gameplay picking = CPU raycast** against the BVH on the Game Thread. Zero GPU involvement, zero latency, and it can hit entities the camera culled.
- **Tools/editor picking = on-demand pick pass**, scissored to a few pixels around the cursor. Each geometry path has an ID-output shader variant writing a tagged `PickId` (u32: 2-bit source tag + 30-bit payload — the mesh path writes the instance-slot index into the shared `InstanceData` buffer, which resolves uniquely to draw + instance; the volume path writes the entity index). Results return through the readback ring (~2-frame latency, fine for tools). The CPU resolves PickId → entity/material and **validates the entity generation**, so a pick landing after a despawn misses cleanly instead of hitting a recycled slot.
- **Debug views** (entity/material heatmaps) run the same pass full-screen, only while active.

Material identity has no dedicated storage anywhere — it is derivable: PickId → draw → `material_gpu_id`.

### 5. Shadow Pass

The engine renders depth from the perspective of each shadow-casting light into a shadow atlas, using the per-shadow-view draw lists produced during culling — shadow views see geometry the main camera culled.

Two kinds of geometry feed each shadow view:

- **The shared mesh stream.** Every `MeshDrawCommand` flagged `CAST_SHADOW` renders into the atlas — regardless of which backend emitted it (the Voxel Plugin's extracted-mesh tiers included).
- **`ShadowCaster` participants.** Backends whose geometry has no triangle form implement the `ShadowCaster` participation trait to render depth into a given shadow view themselves (e.g., raymarched volume depth).

Light types:

- **Directional lights** use cascaded shadow maps (CSM) with configurable cascade count (1–4).
- **Point and spot lights** render into atlas sub-regions.
- **Virtual shadow maps** (future) would cache static shadow pages and only re-render where dynamic objects move.

### 6. Volumetrics (Froxel Media)

*(OQ 9 resolution, 2026-08-11.)* Participating media — global fog, local fog volumes, plugin-injected media — is computed in a frustum-aligned voxel grid (**froxels**) and applied wherever depth is known. Pure compute, no capability gates. The classic four-stage pipeline (Assassin's Creed 4 / Frostbite / UE5 Volumetric Fog):

1. **Density injection.** Global exponential height fog (from `EnvironmentParams`) and `FogVolume` entities are splatted into the grid (density, albedo, emission, phase). Injection is a **public contract**: plugins and games register injector passes that add density/emission — the Voxel Plugin's far-tier smoke uses exactly this.
2. **Froxel lighting.** Per froxel: sample the clustered light grid + shadow atlas, evaluate in-scattering with a Henyey-Greenstein phase function.
3. **Temporal blend.** Reproject and blend with the previous frame's froxel volume (jittered sampling).
4. **Integration.** Front-to-back accumulation into a scattering/transmittance volume.

Consumers sample the integrated volume by pixel depth: the lighting pass applies it to all opaque pixels (meshes and raymarched volumes alike — both are in the GBuffer), the sky pass applies the far-field value, and translucent draws sample it in their forward shaders. Default grid ~160×90×64, quality-tiered, Rgba16Float. The froxel volume is **always bound** (cheap when empty), sidestepping the optional-binding question of OQ 2 for fog.

### 7. Lighting Pass (Deferred Shading)

A full-screen compute dispatch evaluates deferred shading by reading the GBuffer, shadow atlas, and light buffer. Each pixel's lighting response is selected by its GBuffer shading model ID — `Standard` is Cook-Torrance PBR; other registered models (toon, foliage, unlit) branch here.

- **Clustered light assignment.** Screen-space tiles × depth slices. Lights are assigned to clusters on the CPU. Each cluster stores up to 32 light indices.
- **Shadow evaluation.** Percentage-closer filtering (PCF) samples the shadow atlas per light.
- **Pluggable indirect inputs.** The lighting pass declares public input slots for an indirect-diffuse (GI) texture, per-light shadow masks, and **sky visibility**. Engine RT passes feed the first two when hardware RT is available — but all are a **public render-graph contract**: a plugin (e.g., the Voxel Plugin's SVO-traced GI and sky visibility) or a future software-GI tier feeds the same slots without touching the lighting pass. This is the GI upgrade path: each slot progressively supersedes the fallback below it.
- **Indirect diffuse chain** *(OQ 11)*: GI input slot when fed (RT GI, plugin GI) → sky SH9 irradiance × AO × sky visibility → constant ambient.
- **Indirect specular chain** *(OQ 11)*: RT reflections → SSR (**always-on**, not RT-gated) → local reflection probes (when present — deferred feature) → prefiltered sky cubemap × sky visibility.
- **Sky visibility is mandatory.** The environment term — specular *and* diffuse — is always modulated by a sky-visibility factor, so interiors go dark instead of sky-mirrored. Baseline (core): bent-normal/SSAO-derived specular occlusion — screen-space, no authoring, works for any game. Upgrade (public slot): the Voxel Plugin traces directional sky visibility against the SVO — exact and destruction-proof (carve the roof open; visibility updates the same frame).
- **Fog application.** Sample the integrated froxel volume at each pixel's depth and apply scattering/transmittance (see Volumetrics).
- **Output.** HDR lighting result written to an Rgba16Float texture.

### 8. Ray Tracing (Secondary Effects)

Smallworld follows UE5's hybrid rendering model: rasterization handles primary visibility (what the camera sees), ray tracing handles secondary effects (how light bounces, reflects, and casts shadows). Rasterization writes the GBuffer; ray tracing reads it.

This section is conditional on `Capabilities::ray_query`. **The design targets wgpu's inline ray queries exclusively** — rays are traced from compute shaders via `ray_query`. wgpu exposes no ray-tracing pipelines, no shader binding table, and no hit/any-hit/intersection shaders, so nothing in this design may depend on them; on a hit, shaders receive instance/primitive indices and fetch surface data manually (see RT Global Illumination). When hardware RT is unavailable, the engine falls back to screen-space approximations (SSAO, SSR) or skips the effects entirely. The rest of the pipeline is unchanged — RT passes are optional render graph nodes.

#### Acceleration Structure

Ray tracing requires a spatial index on the GPU so rays can efficiently find intersections. This is a two-level hierarchy maintained by the Render Thread as part of `RenderState` (see Data Structures for the `AccelerationStructure` definition):

- **Bottom-Level Acceleration Structure (BLAS).** One per unique mesh geometry. Built from the vertex/index buffers already in `GpuMeshPool`. Rebuilt only when geometry changes (rare for static meshes, per-frame for skinned/deformable).
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

Indirect lighting from light bounces. Cast rays outward from each GBuffer pixel based on a cosine-weighted hemisphere around the surface normal. `ray_query` returns instance and primitive indices on a hit; the shader then fetches the hit point's surface data manually — via a **surface cache** (radiance cached per surface texel, Lumen-style) or bindless vertex/material buffer access. There are no hit shaders under wgpu; this fetch path is a required piece of the design (see Open Questions).

- **Input:** GBuffer, TLAS, material data (surface cache or bindless fetch).
- **Output:** `rt_gi` — Rgba16Float, indirect diffuse irradiance per pixel.
- **Dispatch:** Half-resolution (one ray per 2×2 quad), spatially and temporally denoised, then upsampled. Full-resolution GI is too expensive for real-time; the denoiser fills in.

##### RT Reflections (`RTReflectionPass`)

For pixels with low roughness, cast a reflection ray based on the GBuffer normal. Hit points are shaded and composited over the specular term.

- **Input:** GBuffer (normal, roughness, depth), TLAS.
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
| `RTGIPass` | SSAO + ambient probe | No bounce lighting, baked or constant ambient |
| `RTReflectionPass` | SSR (screen-space reflections) | Misses off-screen reflections |

If the RT passes aren't registered (because `ray_query` is false), the lighting pass's RT input slots go unfed and the fallback terms are used.

The GI fallback is a **conscious v1 quality cliff**: UE5's Lumen degrades through a *software* ray-tracing tier (distance fields) before reaching this point. Smallworld core does not assume any particular scene structure — the SVO belongs to the Voxel Plugin, which core cannot depend on — so a general software-GI tier (SDF scene or screen-space GI) is deferred; see Open Questions. Voxel-heavy games get a better fallback from the Voxel Plugin's SVO-traced GI, delivered through the lighting pass's public input slots.

### 9. Sky & Atmosphere

Rendered into the HDR target where depth equals the far plane. Atmosphere scattering, procedural sky, or skybox cubemap. Applies the froxel volume's far-field scattering for atmospheric consistency with fogged geometry.

#### Environment Capture & IBL

*(OQ 11 resolution, 2026-08-11.)* The sky is also the engine's image-based light source. The environment pipeline maintains three artifacts:

- **Prefiltered specular cubemap.** The sky (procedural or HDRI, per `EnvironmentParams::sky`) is captured to a cubemap and GGX-prefiltered into a roughness → mip chain.
- **SH9 irradiance.** The same capture projected onto 9 spherical-harmonic coefficients — the diffuse ambient term.
- **Split-sum BRDF LUT.** Generated once at startup.

Captures update amortized (one face or one prefilter mip per frame), so time-of-day costs a bounded slice of the frame budget. Consumers: the lighting pass (indirect chains), forward transparents (same prefiltered set), and the froxel lighting's ambient term. Local `ReflectionProbe`s (deferred — see Core Engine Components) reuse this exact capture/prefilter machinery at probe positions via aux views.

### 10. Transparency

*(OQ 9 resolution, 2026-08-11.)* Objects with alpha blending render in a forward pass over the completed opaque image (lighting + sky). They cannot write to the GBuffer.

- **Clustered Forward+ lighting.** Transparent fragments shade with the same Cook-Torrance BRDF, the same clustered light grid, the same shadow atlas, and the same registered shading models as the deferred path — one light structure, two consumers, so lighting matches across the opaque/transparent boundary. The specular environment term is the same prefiltered sky set × sky visibility (see Environment Capture & IBL).
- **Refraction.** Glass and water sample `scene_color_copy` — an HDR snapshot taken after lighting + sky. A pass cannot sample the target it blends into.
- **Fog.** Each transparent fragment samples the integrated froxel volume at its depth.
- **Sorting.** Back-to-front per draw. No OIT in v1 — interpenetration artifacts are accepted (the shipped-game norm); weighted-blended OIT is a future quality knob.
- **Translucent media ≠ surface transparency.** Smoke/fire-like media renders in this stage but through different machinery: near-field hero media in plugin-owned raymarch passes (front-to-back march lit by the cluster grid + shadow atlas, composited by transmittance against scene depth — reads `depth`, never writes it), far-field media injected into the froxel grid. The opaque `VolumePass` is never used for media.

### 11. Post-Processing

Camera-lens effects applied to the HDR image:

- **Temporal Anti-Aliasing (TAA).** Jittered projection + motion vectors + history buffer to resolve sub-pixel detail and reduce aliasing.
- **Bloom.** Downsample bright regions, blur, composite back.
- **Tone mapping.** HDR → SDR/display HDR via ACES or Reinhard.
- **Color grading.** Final LUT application.

### 12. Present

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

    // Cull retained data for one view (frustum, HZB, distance). Called once per view.
    fn cull(&self, view: &ViewParams, hzb: Option<&wgpu::TextureView>, out: &mut ViewDrawList);

    // Register the render passes this geometry type contributes.
    fn register_passes(&self, graph: &mut RenderGraph);

    // Optional pass participation (see below).
    fn shadow_caster(&self) -> Option<&dyn ShadowCaster> { None }
    fn tlas_contributor(&self) -> Option<&dyn TlasContributor> { None }
}

// Per-view culling output: visible members of the shared mesh stream, plus
// backend-defined visible-set payloads consumed by the backend's own passes.
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

The engine ships two backends. Both use the same extractor/renderer traits a game would.

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
    upserts: Vec<(DrawId, MeshDrawCommand)>,  // spawned or changed draws
    removes: Vec<DrawId>,                     // despawned draws
}
```

- **Owned and `Send`.** No references, no borrowed lifetimes. Once sent through the channel, the Game Thread is free to mutate the World.
- **Delta-driven.** The extract step walks the `ChangeTracker`'s dirty sets; unchanged entities produce nothing. The retained `RenderScene` carries everything else across frames.
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
    UploadMesh     { gpu_id: GpuId, vertices: Vec<Vertex>, indices: Vec<u32>, bounds: AABB },
    UploadTexture  { gpu_id: GpuId, pixels: Vec<u8>, width: u32, height: u32,
                     format: TextureFormat, mip_count: u32 },  // pixels holds all mips, tightly packed
    UpdateMaterial { gpu_id: GpuId, props: MaterialGpuProps },
    Free           { gpu_id: GpuId, kind: ResourceKind },
}
```

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
}
```

This is the equivalent of UE5's `FMeshDrawCommand` — a fully stateless draw description that can be sorted, merged, and cached. Because the render side retains the mesh store across frames, static commands genuinely *are* cached: sorted batch lists for static geometry are rebuilt only when the store changes, not per frame. Unlike UE5, we don't have the intermediate `FMeshBatch` layer as a data structure — its cross-backend role is played by the shared mesh stream itself; the extract step produces final draw commands directly.

#### VolumeDrawCommand

The Voxel Plugin's custom-lane draw data — carried in its backend delta payload and consumed only by `VolumePass`. (Its extracted-mesh tiers travel the shared mesh stream as ordinary `MeshDrawCommand`s.)

```rust
struct VolumeDrawCommand {
    volume_id:       EntityId,
    bounds:          AABB,
    lod_level:       u8,
    brick_residency: BrickResidencyInfo,
}
```

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

### Scripting Runtime

For gameplay logic that needs to be iterable without recompilation, smallworld provides a sandboxed scripting runtime. Scripts attach to entities as components and receive lifecycle callbacks.

```rust
trait ScriptInstance: Send {
    fn init(&mut self, entity: EntityId, ctx: &mut ScriptContext);
    fn update(&mut self, entity: EntityId, ctx: &mut ScriptContext, dt: f32);
    fn on_event(&mut self, entity: EntityId, ctx: &mut ScriptContext, event: &dyn Event);
    fn shutdown(&mut self, entity: EntityId, ctx: &mut ScriptContext);
}

struct ScriptContext<'a> {
    world:  &'a mut World,
    input:  &'a Input,
    time:   &'a Time,
    events: &'a mut EventBus,
}
```

Scripts can read and write components, query the world, and emit events — but they cannot access GPU resources, raw pointers, or unsafe Rust. The script runtime (Rhai, Lua, or WASM) provides the sandbox; the engine provides the `ScriptContext` API surface.

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
        const VISIBLE  = 0x02;  // participates in rendering
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

##### Camera

```rust
struct Camera {
    projection: Projection,
    target:     RenderTargetRef,   // Screen, or an offscreen texture (RTT, probes)
    priority:   i32,               // ordering among active cameras (split-screen, PiP)
    active:     bool,
}

enum Projection {
    Perspective  { fov_y: f32, near: f32, far: f32 },
    Orthographic { height: f32, near: f32, far: f32 },
}

enum RenderTargetRef { Screen, Texture(ResourceHandle<RenderTexture>) }
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
    source:          Box<dyn VolumeSource>,
    bounds:          AABB,
    lod_policy:      LodPolicy,
    stream_priority: StreamPriority,
}

trait VolumeSource: Send + Sync {
    fn generate(&self, coord: BrickCoord, world_min: Vec3) -> Option<BrickData>;
    fn bounds(&self) -> AABB;
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
- `fixed_update` — called at a fixed rate (default 60 Hz, configurable). Physics integration, network tick, anything that needs deterministic timestep. May run 0–N times per frame depending on accumulated time.
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

    // Render feedback (see Frame Pipeline — Render-to-Game Feedback)
    fn feedback(&self) -> Option<&FrameFeedback>;
    fn gpu_frame_time(&self) -> f32;
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
    vsync:           bool,
    fixed_timestep:  f32,        // default 1/60
    pipeline_mode:   PipelineMode,
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

A `World` contains all entities for the current level. Level transitions swap the entire World. For streaming open worlds, the engine supports region-based loading — entities within a geographic region are spawned and despawned based on camera distance.

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

Compound assets (glTF, custom scene format) are loaded through the `AssetServer` and produce `LoadedScene` values that bulk-insert entities.

#### Entity Hierarchy

Entities can form parent-child trees. A character entity might parent its weapon, particle emitters, and audio sources. When the parent moves, children inherit the transform. When the parent is despawned, children are despawned recursively.

This is equivalent to UE5's `SetupAttachment` component tree — but flattened into a simple parent ID on the entity rather than a component hierarchy.

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

A typed event bus for decoupled communication between game systems. Events live for one frame and are cleared automatically.

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

## Frame Lifecycle

The complete sequence of a single frame, showing which thread owns each phase.

### Game Thread

| Phase | Action |
|-------|--------|
| **INPUT** | Main thread accumulates window events into `Input` snapshot |
| **FIXED** | `App::fixed_update()` runs 0–N times at fixed timestep |
| **UPDATE** | `App::update()` runs once. Game systems mutate World |
| **LATE** | Engine systems: hierarchy propagation, bounds recomputation, streaming demand |
| **EXTRACT** | Diff `&World` via `&ChangeTracker` into a `FramePacket` (views, lights, deltas, resource ops), send through channel |
| **CLEAR** | Clear change tracker. Game thread free for next frame |

### Render Thread

| Phase | Action |
|-------|--------|
| **RECEIVE** | Block on channel, receive `FramePacket` |
| **PREPARE** | Apply `ResourceOp`s and scene deltas — upload resources, update the retained `RenderScene` |
| **CULL** | Derive shadow views; collect TLAS instances (pre-cull); per-view frustum + occlusion culling; produce sorted, batched draw lists |
| **RECORD** | Render graph executes: each pass records GPU commands |
| **SUBMIT** | `queue.submit()` — command buffers sent to hardware |
| **PRESENT** | Swapchain present. Send `FrameFeedback`. Loop back to RECEIVE |

### Ownership Boundaries

| Data | Owner | Crosses boundary as |
|------|-------|---------------------|
| World, Components | Game Thread | Read-only in extract |
| FramePacket (deltas + views) | Produced by Game, consumed by Render | Owned value through channel (Game → Render) |
| RenderScene (retained draw data) | Render Thread | Never — updated only by applying packet deltas |
| FrameFeedback | Produced by Render, consumed by Game | Owned value through channel (Render → Game, ~2-frame lag; GPU data via readback ring) |
| GPU resources | Render Thread | Never — game code uses handles |
| Assets (CPU) | AssetServer (Game Thread) | Copied into `ResourceOp` at extract (transport under review — Open Questions) |
| Input | Main Thread | Snapshot borrowed by game tick |
| Audio commands | Collected on Game Thread | Drained by audio server each frame |

---

## Open Questions

The complete open-decision backlog from the 2026-08-11 review round. Items 1–6 are design
decisions needing a follow-up discussion before implementation; items 7–20 need at least a
documented stance (some are full subsystem designs); items 21–23 are doc/code reconciliation.
The plan: resolve them one by one, in discussion — no implementation issues until a discussion
lands.

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
3. **Surface cache.** `ray_query` hits return instance/primitive indices only; RT GI and
   reflections need a hit-point material fetch — surface cache (Lumen-style) vs. bindless
   vertex/material access. Required before RT GI/reflections can be implemented.
4. **Core software-GI tier.** v1 ships the SSAO + sky-IBL-with-visibility fallback as a conscious
   cliff (per OQ 11). Does core eventually grow a scene-agnostic software tier (SDF scene,
   screen-space GI), or does plugin-provided GI (the Voxel Plugin's SVO tracing) cover the cases
   that matter? The lighting chains from OQ 11 already reserve the public slots (GI, sky
   visibility, reflections) this tier would feed — whatever the answer, no shader rework.
5. **Asset payload transport.** `ResourceOp` currently deep-copies vertex/pixel data into the
   channel. Options: `Arc<[u8]>` of immutable asset bytes (thread-safe — the ownership rule
   targets shared *mutable* state), or pre-populated staging buffers handed off by handle.
6. **Gameplay-layer semantics.** Event bus buffering (same-frame visibility vs. double-buffered),
   script access to `World` (command buffers vs. exclusive storage), and fixed-timestep transform
   interpolation for rendering. None block the render architecture; all block gameplay API
   stability.
7. **GPU-driven rendering.** Adopt or consciously defer GPU culling, indirect draws, and
   bindless resources. The contracts no longer preclude it (`MeshDrawCommand` is
   instancing-capable, pools are dense), but the decision itself was never made — and
   "Nanite-for-bricks" (GPU brick culling, per-brick LOD on GPU) is the Voxel Plugin's stake in
   it.
8. **Frame pacing & latency control.** Beyond `PipelineMode::Lockstep` there is no story: no
   GPU-bound throttling policy, no maximum-frames-in-flight control at the present layer, no
   Reflex-style pacing. Decide what v1 ships and what the config surface looks like.
9. **[RESOLVED 2026-08-11] Translucency lighting & volumetrics.** Three-part resolution:
   (1) transparent surfaces shade via Clustered Forward+ reusing the deferred light grid, shadow
   atlas, and shading models, with refraction from `scene_color_copy` and no OIT in v1;
   (2) participating media lives in a core froxel volumetric system with a **public injector
   contract** (`FogVolume` component + `EnvironmentParams` height fog); (3) translucent voxel
   media is plugin-side, two tiers — froxel injection far, raymarched media pass in the
   transparency stage near; the opaque `VolumePass` never renders media. Specs: Volumetrics and
   Transparency stages. Dependencies flagged into OQ 11 (env/fog params, translucent specular)
   and OQ 12 (upscaling vs. froxel/half-res media resolution).
10. **Seamless LOD transitions.** LOD *selection* is specified; *transitions* are not:
    cross-fade/dither for meshes, brick-resolution blending and the extracted-mesh↔raymarch
    handoff for the Voxel Plugin. Decide whether the backend contract needs explicit transition
    hooks or each backend owns it privately.
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
12. **Post chain completeness.** Auto-exposure (eye adaptation) and temporal upscaling
    (TSR/FSR-class — the biggest per-pixel performance lever for expensive raymarching) are both
    absent; both restructure the post-processing section (internal vs. display resolution). The
    upscaling decision must account for the froxel grid and half-res hero-media reconstruction
    (OQ 9 rider).
13. **Decals.** No story. Deferred decals interact directly with the GBuffer contract; decide
    in or out for v1.
14. **Skinning.** Where skinning runs (compute pre-skin vs. vertex shader) and how skinned
    vertices feed the depth pre-pass, motion vectors, and per-frame BLAS refit consistently.
15. **Resize / device-lost / teardown.** Swapchain recreation crosses the thread boundary;
    `Engine::run() -> !` plus `App::shutdown` implies a drain/GPU-idle ordering that is
    unspecified. Define the lifecycle protocol.
16. **Physics architecture.** Engine choice (e.g., rapier), `Transform` sync, fixed-step
    ownership — plus worker-pool priorities: game physics jobs and render-critical culling jobs
    share one rayon pool today, with no protection against priority inversion.
17. **Streaming.** Principle 5 promises budget arbitration; `BrickResidencyInfo` and
    `StreamPriority` are name-dropped; a World Partition-analog design is missing. Deserves a
    full section of its own.
18. **UI.** No story (immediate-mode overlay, egui integration, retained widget tree?). A
    complete game engine needs at least a stance.
19. **Networking.** Not mentioned anywhere — even "out of scope for v1" needs saying, with the
    architectural implications named (determinism, fixed-tick replication hooks).
20. **Save / serialization.** Asset descriptors spawn entities; serializing a live `World`
    (save games, editor scenes) is undesigned.
21. **Voxel Plugin data ownership.** `VolumeRenderer` holds `Box<dyn VolumeSource>` — behavior
    inside a "plain data" component — and `VolumeDrawCommand.brick_residency` is produced at
    extract while residency is streaming/GPU-side state. Decide which side owns residency; fold
    into the Voxel Plugin / streaming design (with 17).
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
