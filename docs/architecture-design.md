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

- **Step 1: Game Thread (Frame N).** The engine processes game logic, physics, animation, and input. At the end of the tick, the extract step snapshots the World into a self-contained `FramePacket` and sends it through a bounded channel.
- **Step 2: Render Thread (Frame N, one step behind).** The Render Thread receives the `FramePacket`, processes GPU resource updates, then executes the render graph to produce the final image.

This introduces one frame of input latency (~16 ms at 60 fps) in exchange for doubled throughput. Same tradeoff as UE5.

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
| **Render Thread** (dedicated) | GpuContext, RenderGraph, GPU resource pools, render targets | Receives `FramePacket`; sends `FrameFeedback` back |
| **Worker Pool** (rayon, work-stealing) | Nothing persistent — borrows work items | Scoped tasks with `join()` / `parallel_for()` |

The Worker Pool is shared between both threads. The Game Thread uses it for physics broadphase, animation sampling, and streaming demand computation. The Render Thread uses it for frustum culling, draw call sorting, and batch generation.

### Render-to-Game Feedback

The pipeline is not one-directional. The Render Thread sends a `FrameFeedback` back to the Game Thread after each frame completes. This travels through a separate channel and arrives two frames later — the Game Thread reads feedback from frame N-2 while processing frame N. Never a synchronous wait.

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
    frame_index:    u64,
    gpu_time:       GpuTimingFeedback,
    occlusion:      OcclusionFeedback,
    readback:       Vec<ReadbackResult>,
}

struct GpuTimingFeedback {
    total_gpu_ms:    f32,
    pass_timings:    Vec<(String, f32)>,  // per render pass
    gpu_memory_used: u64,
}

struct OcclusionFeedback {
    visible_mesh_count:   u32,
    visible_volume_count: u32,
    culled_count:         u32,
}

enum ReadbackResult {
    OcclusionQuery { entity: EntityId, visible_samples: u32 },
    ComputeResult  { tag: u32, data: Vec<u8> },
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

In lockstep mode, feedback arrives from the immediately preceding frame instead of N-2, but the game thread stalls until the render thread finishes — the same bottleneck UE5 documents with `r.OneFrameThreadLag 0`.

---

## The Render Thread

The Render Thread receives a `FramePacket` each frame and translates it into GPU work through a structured sequence of passes organized by a render graph.

### 1. Receive & Prepare

The Render Thread blocks on the channel until a `FramePacket` arrives. It then processes any `ResourceOp` entries — uploading new meshes and textures to GPU pools, updating material uniform data, and freeing resources for despawned entities.

This is the only point where the Render Thread mutates its persistent state (GPU resource caches). Everything after this is read-only traversal of the packet.

### 2. Visibility & Culling

Before anything is drawn, the engine determines what the camera can see. Each registered `GeometryBackend` runs its own culling logic via `backend.cull()`, allowing geometry types to implement specialized culling strategies (e.g., octree traversal for volumes vs. flat AABB tests for meshes).

- **Frustum culling.** Test every draw command's world-space AABB against the camera's six frustum planes. Parallelized across the worker pool.
- **Occlusion culling (HZB).** Using the Hierarchical Z-Buffer from the previous frame's depth, test remaining objects to discard those hidden behind large occluders.
- **LOD selection.** For volumes and meshes with LOD levels, select the appropriate detail tier based on screen-space size or distance. Each backend owns its LOD strategy.
- **Sort & batch.** Opaque draws are sorted front-to-back (minimize overdraw), transparent draws back-to-front. Draws sharing the same pipeline state are batched.

### 3. Depth Pre-Pass

Establishes the scene's depth early to prevent overdraw in the full GBuffer pass.

- **Early Z.** Opaque meshes render depth-only to the Z-buffer.
- **HZB construction.** The depth buffer is downsampled into a mip chain for next-frame occlusion queries.

### 4. GBuffer Pass (Geometry)

Visible opaque surfaces write their material properties to the Geometry Buffer. Smallworld uses deferred rendering, separating geometry from lighting.

| GBuffer Target | Format | Data |
|----------------|--------|------|
| Albedo | Rgba8UnormSrgb | Base color (RGB) + flags (A) |
| Normal | Rgba16Float | World-space normals (octahedral encoded) |
| Material | Rgba8Unorm | Roughness, metallic, reflectance, AO |
| Emissive | Rgba16Float | Self-illumination (RGB) + intensity |
| Velocity | Rg16Float | Per-pixel motion vectors (TAA, motion blur) |
| Entity ID | R32Uint | Entity/material ID for debug and picking |
| Depth | D32Float | Z-buffer |

Both rendering paths write to this same GBuffer:

- **Rasterized meshes** write via traditional vertex/fragment shaders through the `GBufferPass`.
- **Raymarched volumes** write via compute shaders through the `VolumePass`, reading depth to composite correctly with rasterized geometry.

The lighting pass and everything downstream never knows which path produced a given pixel.

### 5. Shadow Pass

The engine renders depth from the perspective of each shadow-casting light into a shadow atlas.

- **Directional lights** use cascaded shadow maps (CSM) with configurable cascade count (1–4).
- **Point and spot lights** render into atlas sub-regions.
- **Virtual shadow maps** (future) would cache static shadow pages and only re-render where dynamic objects move.

### 6. Lighting Pass (Deferred Shading)

A full-screen compute dispatch evaluates Cook-Torrance PBR shading by reading the GBuffer, shadow atlas, and light buffer.

- **Clustered light assignment.** Screen-space tiles × depth slices. Lights are assigned to clusters on the CPU. Each cluster stores up to 32 light indices.
- **Shadow evaluation.** Percentage-closer filtering (PCF) samples the shadow atlas per light.
- **Output.** HDR lighting result written to an Rgba16Float texture.

### 7. Ray Tracing (Secondary Effects)

Smallworld follows UE5's hybrid rendering model: rasterization handles primary visibility (what the camera sees), ray tracing handles secondary effects (how light bounces, reflects, and casts shadows). Rasterization writes the GBuffer; ray tracing reads it.

This section is conditional on `Capabilities::ray_query`. When hardware RT is unavailable, the engine falls back to screen-space approximations (SSAO, SSR) or skips the effects entirely. The rest of the pipeline is unchanged — RT passes are optional render graph nodes.

#### Acceleration Structure

Ray tracing requires a spatial index on the GPU so rays can efficiently find intersections. This is a two-level hierarchy maintained by the Render Thread as part of `RenderState`:

- **Bottom-Level Acceleration Structure (BLAS).** One per unique mesh geometry. Built from the vertex/index buffers already in `GpuMeshPool`. Rebuilt only when geometry changes (rare for static meshes, per-frame for skinned/deformable).
- **Top-Level Acceleration Structure (TLAS).** One per frame. References all BLAS instances with their world transforms. Rebuilt every frame from the `FramePacket`'s draw commands — each `MeshDrawCommand` becomes a TLAS instance entry.

```rust
struct AccelerationStructure {
    blas_cache:  HashMap<GpuId, BlasEntry>,
    tlas:        wgpu::Tlas,
    tlas_dirty:  bool,
}

struct BlasEntry {
    blas:        wgpu::Blas,
    mesh_gpu_id: GpuId,
    generation:  u32,         // rebuilt when mesh geometry changes
}
```

The TLAS build is a GPU operation — the Render Thread records it as a command before the RT passes execute. Cost is proportional to instance count, not triangle count, so it scales well.

Volume geometry poses a challenge: voxel data doesn't have a triangle representation to feed into BLAS construction. Two strategies:

- **Extracted mesh BLAS.** If the `VolumeBackend` extracts meshes (marching cubes / dual contouring), those meshes get BLAS entries like any other mesh. Works today, coarser than the actual voxel data.
- **SVO traversal in ray-any-hit.** Keep the SVO on the GPU and trace against it using a custom intersection shader. More accurate but requires `ray_tracing_pipeline` support beyond basic `ray_query`. This is the long-term path.

#### RT Passes

RT passes are standard `RenderPass` implementations that read the GBuffer and trace rays against the TLAS. They write to dedicated targets consumed by the lighting pass.

##### RT Shadows (`RTShadowPass`)

For each pixel in the GBuffer, cast one shadow ray toward each light source through the TLAS. Produces a per-light shadow mask — a binary (or soft-penumbra) occlusion value per pixel. Replaces or supplements the rasterized shadow atlas for lights that opt in.

- **Input:** GBuffer (depth, normal, position reconstructed from depth), TLAS, light buffer.
- **Output:** `rt_shadow_mask` — Rgba8Unorm, one channel per shadow-casting light (up to 4 per tile; overflow falls back to shadow atlas).
- **Dispatch:** Full-screen compute, 8×8 workgroups. One ray per pixel per light. Denoised temporally.

##### RT Global Illumination (`RTGIPass`)

Indirect lighting from light bounces. Cast rays outward from each GBuffer pixel based on a cosine-weighted hemisphere around the surface normal. Hit points sample their own material (via the BLAS hit shader or a surface cache) to compute bounced radiance.

- **Input:** GBuffer, TLAS, material data (surface cache or hit shaders).
- **Output:** `rt_gi` — Rgba16Float, indirect diffuse irradiance per pixel.
- **Dispatch:** Half-resolution (one ray per 2×2 quad), spatially and temporally denoised, then upsampled. Full-resolution GI is too expensive for real-time; the denoiser fills in.

##### RT Reflections (`RTReflectionPass`)

For pixels with low roughness, cast a reflection ray based on the GBuffer normal. Hit points are shaded and composited over the specular term.

- **Input:** GBuffer (normal, roughness, depth), TLAS.
- **Output:** `rt_reflections` — Rgba16Float, reflected radiance per pixel.
- **Dispatch:** Selective — only pixels below a roughness threshold. Fallback to screen-space reflections (SSR) for rough surfaces or when RT is unavailable.

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

The shader branches on whether RT targets are bound. When RT shadows are available for a light, they replace the shadow atlas sample for that light. RT GI adds to the ambient/indirect term. RT reflections replace or blend with the specular term based on roughness.

This is pure additive integration — the rasterization pipeline produces a complete image on its own. RT passes improve quality when available but nothing breaks without them.

#### Render Targets (RT)

```rust
// Added to RenderTargets when Capabilities::ray_query is true
struct RTTargets {
    shadow_mask:  wgpu::Texture,  // Rgba8Unorm — per-light RT shadow
    gi:           wgpu::Texture,  // Rgba16Float — indirect diffuse
    reflections:  wgpu::Texture,  // Rgba16Float — specular reflections
    history_gi:   wgpu::Texture,  // Rgba16Float — temporal accumulation for denoiser
    history_refl: wgpu::Texture,  // Rgba16Float — temporal accumulation for denoiser
}
```

#### Fallback Path (No Hardware RT)

When `ray_query` is unavailable, the engine uses screen-space approximations in the same render graph slots:

| RT Pass | Fallback | Quality tradeoff |
|---------|----------|------------------|
| `RTShadowPass` | Shadow atlas only (rasterized CSM/atlas) | No soft penumbra from RT, same shadows as baseline |
| `RTGIPass` | SSAO + ambient probe | No bounce lighting, baked or constant ambient |
| `RTReflectionPass` | SSR (screen-space reflections) | Misses off-screen reflections |

The render graph handles this naturally — if the RT passes aren't registered (because `ray_query` is false), the lighting pass simply doesn't bind their targets and uses the fallback terms.

### 8. Sky & Atmosphere

Rendered into the HDR target where depth equals the far plane. Atmosphere scattering, procedural sky, or skybox cubemap.

### 9. Transparency

Objects with alpha blending are rendered in a forward pass, sorted back-to-front. They sample the HDR lighting buffer for correct compositing but cannot write to the GBuffer.

### 10. Post-Processing

Camera-lens effects applied to the HDR image:

- **Temporal Anti-Aliasing (TAA).** Jittered projection + motion vectors + history buffer to resolve sub-pixel detail and reduce aliasing.
- **Bloom.** Downsample bright regions, blur, composite back.
- **Tone mapping.** HDR → SDR/display HDR via ACES or Reinhard.
- **Color grading.** Final LUT application.

### 11. Present

The final image is blitted to the swapchain surface. The Render Thread loops back to receive the next `FramePacket`.

---

## Customizing the Render Pipeline

Smallworld's rendering architecture is modular at five levels, mirroring UE5's customization depth but expressed in Rust traits rather than C++ inheritance. The guiding principle: **the engine's own voxel volume support is a geometry backend plugin, not a special case.** It uses the same `GeometryBackend` trait a game would use to add GPU particles, procedural terrain, or SDF shapes. If the API isn't powerful enough for voxels, it isn't powerful enough for games.

### 1. Custom Geometry Types (`GeometryBackend`)

The deepest customization point. A `GeometryBackend` defines a new kind of renderable — its game-side component, how it extracts into draw commands, how it manages GPU resources, and which render passes it needs.

```rust
trait GeometryBackend: Send + Sync {
    fn name(&self) -> &str;

    // Which component type does this backend process?
    fn component_id(&self) -> TypeId;

    // Extract: read World components, produce draw commands for the Render Thread
    fn extract(
        &self,
        world: &World,
        changes: &ChangeTracker,
        camera: &CameraParams,
    ) -> Box<dyn DrawCommandSet>;

    // Cull: filter commands by visibility (frustum, HZB, distance)
    fn cull(
        &self,
        commands: &mut dyn DrawCommandSet,
        camera: &CameraParams,
        hzb: Option<&wgpu::TextureView>,
    );

    // Prepare: upload/update GPU resources this geometry type needs
    fn prepare(&mut self, commands: &dyn DrawCommandSet, state: &mut RenderState);

    // Register the render passes this geometry type contributes
    fn register_passes(&self, graph: &mut RenderGraph);
}
```

`DrawCommandSet` is an opaque, type-erased container. Each backend defines its own concrete draw command struct; render passes downcast to access it.

```rust
trait DrawCommandSet: Send + Sync + Any {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn bounds(&self) -> &[AABB];        // per-command world bounds, for culling
    fn as_any(&self) -> &dyn Any;       // downcast in render passes
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
```

#### Built-in Backends

The engine ships two backends. Both use the same `GeometryBackend` trait a game would.

| Backend | Component | Draw Command | Passes | Notes |
|---------|-----------|-------------|--------|-------|
| `MeshBackend` | `MeshRenderer` | `MeshDrawCommand` | `DepthPrepass`, `GBufferPass`, `ShadowPass`, `TransparencyPass` | Standard triangle rasterization |
| `VolumeBackend` | `VolumeRenderer` | `VolumeDrawCommand` | `VolumePass` | Compute raymarching or mesh extraction; pluggable strategy per LOD tier |

Both converge at the same GBuffer — the lighting pass and everything downstream is backend-agnostic.

#### Registering a Custom Backend

Games register backends at init time. The engine integrates them into the extract → cull → prepare → render pipeline automatically.

```rust
impl GameContext<'_> {
    fn register_geometry_backend(&mut self, backend: impl GeometryBackend + 'static);
}

// Example: a game adds GPU particle rendering
struct ParticleBackend { /* ... */ }

impl GeometryBackend for ParticleBackend {
    fn name(&self) -> &str { "particles" }
    fn component_id(&self) -> TypeId { TypeId::of::<ParticleEmitter>() }
    fn extract(&self, world: &World, changes: &ChangeTracker, camera: &CameraParams)
        -> Box<dyn DrawCommandSet> { /* ... */ }
    fn cull(&self, commands: &mut dyn DrawCommandSet, camera: &CameraParams,
        hzb: Option<&wgpu::TextureView>) { /* ... */ }
    fn prepare(&mut self, commands: &dyn DrawCommandSet, state: &mut RenderState) { /* ... */ }
    fn register_passes(&self, graph: &mut RenderGraph) { /* ... */ }
}
```

### 2. Custom Draw Processing (`DrawProcessor`)

If you want to modify how a standard pass handles draws — custom sorting, per-draw filtering, shader binding overrides — without writing an entire pass from scratch, you provide a `DrawProcessor`.

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

Games need materials beyond the built-in PBR model — toon shading, water, foliage wind, hologram effects. The engine provides a shader composition system where games supply WGSL fragments that plug into defined insertion points.

```rust
struct CustomMaterial {
    base:            MaterialDef,            // PBR properties still available
    fragment_shader: ShaderFragment,         // custom WGSL fragment
    uniforms:        Vec<(String, UniformValue)>,  // custom uniform data
    textures:        Vec<(String, AssetHandle<TextureAsset>)>,
}

struct ShaderFragment {
    source: String,          // WGSL code
    entry_point: String,     // function name
    stage: ShaderStage,      // Fragment, Vertex, or Compute
}
```

The engine composes the final shader by concatenating the standard GBuffer output code with the custom fragment. Custom materials still write to the same GBuffer — they control *how* the albedo, normal, roughness, etc. are computed, not *where* they go.

---

## Data Structures

### 1. The Extract System (Bridging the Threads)

Smallworld's equivalent of UE5's Scene Proxy system. Instead of maintaining persistent mirror objects on the Render Thread, we send a self-contained `FramePacket` each frame. This is simpler and more Rust-friendly — no shared ownership, no lifetime entanglement.

The `FramePacket` is extensible — it carries a `DrawCommandSet` per registered `GeometryBackend` rather than hardcoding specific draw command types.

#### FramePacket

The complete render-thread-safe snapshot of everything needed to draw one frame.

```rust
struct FramePacket {
    camera:       CameraParams,
    draw_sets:    Vec<Box<dyn DrawCommandSet>>,  // one per registered GeometryBackend
    lights:       Vec<LightParams>,
    environment:  EnvironmentParams,
    resource_ops: Vec<ResourceOp>,
    frame_index:  u64,
}
```

- **`Send` and owned.** No references, no `Arc`, no borrowed lifetimes. Once sent through the channel, the Game Thread is free to mutate the World.
- **Extensible.** Each registered `GeometryBackend` contributes one `DrawCommandSet` to the packet. The engine doesn't know or care what's inside — it's opaque until a render pass downcasts it.
- **Change-driven.** The extract step reads the World's `ChangeTracker` to identify dirty entities. Unchanged entities reuse their draw commands from the previous packet.
- **Read-only extraction.** The extract function borrows `&World`, never `&mut World`.

#### CameraParams

```rust
struct CameraParams {
    view:            Mat4,
    projection:      Mat4,
    view_projection: Mat4,
    position:        Vec3,
    frustum_planes:  [Vec4; 6],
    near:            f32,
    far:             f32,
    jitter:          Vec2,       // TAA sub-pixel jitter
}
```

#### ResourceOp

When the game adds, modifies, or removes assets, the extract step encodes these as resource operations for the Render Thread.

```rust
enum ResourceOp {
    UploadMesh     { gpu_id: GpuId, vertices: Vec<Vertex>, indices: Vec<u32>, bounds: AABB },
    UploadTexture  { gpu_id: GpuId, pixels: Vec<u8>, width: u32, height: u32, format: TextureFormat },
    UpdateMaterial { gpu_id: GpuId, props: MaterialGpuProps },
    Free           { gpu_id: GpuId, kind: ResourceKind },
}
```

### 2. The Mesh Drawing Pipeline

#### MeshDrawCommand

The render-ready description of a single draw. Fully resolved — no handles to chase, no indirection.

```rust
struct MeshDrawCommand {
    mesh_gpu_id:       GpuId,       // index into GpuMeshPool
    material_gpu_id:   GpuId,       // index into GpuMaterialPool
    world_matrix:      Mat4,
    prev_world_matrix: Mat4,        // for motion vectors
    bounds:            AABB,        // world-space, for culling
    flags:             DrawFlags,   // shadow casting, double-sided, alpha mode
}
```

This is the equivalent of UE5's `FMeshDrawCommand` — a fully stateless draw description that can be sorted, merged, and cached. Unlike UE5, we don't have the intermediate `FMeshBatch` layer; the extract step produces final draw commands directly.

#### VolumeDrawCommand

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

The Render Thread owns all GPU memory through typed pools. Resources are identified by `GpuId` — an opaque handle that the extract layer maps from game-side handles. When hardware RT is available, the Render Thread also maintains the TLAS/BLAS acceleration structure.

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
    timestamp_query:   bool,
    ray_query:         bool,
    mesh_shader:       bool,
    shader_f16:        bool,
    subgroups:         bool,
    max_buffer_mb:     u32,
    max_texture_dim:   u32,
    min_ubo_alignment: u32,
}
```

#### Acceleration Structure (RT)

Allocated only when `Capabilities::ray_query` is true. Maintained by the Render Thread — rebuilt each frame from the `FramePacket`'s draw commands.

```rust
struct AccelerationStructure {
    blas_cache: HashMap<GpuId, BlasEntry>,
    tlas:       wgpu::Tlas,
}

struct BlasEntry {
    blas:        wgpu::Blas,
    mesh_gpu_id: GpuId,
    generation:  u32,
}
```

#### GPU Resource Pools

```rust
struct GpuMeshPool {
    meshes: HashMap<GpuId, GpuMesh>,
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
    textures: HashMap<GpuId, GpuTexture>,
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
    materials: HashMap<GpuId, GpuMaterialEntry>,
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
    gbuffer_albedo:   wgpu::Texture,  // Rgba8UnormSrgb
    gbuffer_normal:   wgpu::Texture,  // Rgba16Float
    gbuffer_material: wgpu::Texture,  // Rgba8Unorm
    gbuffer_emissive: wgpu::Texture,  // Rgba16Float
    gbuffer_velocity: wgpu::Texture,  // Rg16Float
    gbuffer_id:       wgpu::Texture,  // R32Uint
    hdr:              wgpu::Texture,  // Rgba16Float
    shadow_atlas:     wgpu::Texture,  // D32Float
    hzb:              wgpu::Texture,  // R32Float mip chain

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

### 5. Geometry Backend Convergence

The GBuffer is the unification point. Every registered `GeometryBackend` — built-in or game-defined — writes to the same targets. The lighting pass and everything downstream is backend-agnostic.

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

The `VolumeBackend` itself is pluggable in how it renders — compute raymarching, mesh extraction (marching cubes / dual contouring fed into rasterization), or a hybrid where nearby volumes get full-resolution raymarching and distant volumes get extracted meshes. This is an internal detail of the backend, invisible to the rest of the pipeline.

When RT is available, the full pipeline including optional RT passes looks like:

```
  Backends ──▶ GBuffer ──▶ Shadows ──┐
                   │                  │
                   ├──▶ RT Shadows ───┤
                   ├──▶ RT GI ────────┼──▶ Lighting ──▶ Sky ──▶ Transparency ──▶ Post ──▶ Present
                   └──▶ RT Reflect ───┘
```

RT passes are optional nodes in the graph. The lighting pass binds their outputs when present, falls back to rasterized shadows / SSAO / SSR when absent. The render graph's dependency resolution handles this automatically — unregistered passes simply don't produce their targets, and the lighting shader branches accordingly.

Future raytraced shadows and GI plug in as additional render graph passes (`RTShadowPass`, `RTGIPass`) that read the GBuffer and write to targets consumed by the lighting pass. The existing backends and their passes are unchanged.

---

## Composability & Scripting

UE5's real power is the composability of its Actor-Component model and data-driven design, not the Blueprint visual editor. Smallworld takes the same composability principles and expresses them natively in Rust.

### The Entity-Component Model

Every entity in the world is an ID in a SlotMap. Behavior is assembled by attaching components — plain data structs. There is no `AActor` base class, no `UObject` hierarchy, no reflection macros. Components are registered by type and stored in dense, cache-friendly arrays.

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
    assets: &'a AssetServer,
    audio:  &'a mut AudioCommands,
    events: &'a mut EventBus,
    window: &'a WindowState,
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
    spawned:   HashSet<EntityId>,
    despawned: HashSet<EntityId>,
    dirty:     HashMap<TypeId, HashSet<EntityId>>,  // per-component-type dirty sets
}

impl ChangeTracker {
    fn is_dirty<C: Component>(&self, entity: EntityId) -> bool;
    fn dirty_set<C: Component>(&self) -> &HashSet<EntityId>;
    fn spawned(&self) -> &HashSet<EntityId>;
    fn despawned(&self) -> &HashSet<EntityId>;
}
```

Mutations through `World::get_mut<C>()` automatically mark the component dirty. The change tracker is cleared after the extract step completes.

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
| **EXTRACT** | Read `&World` + `&ChangeTracker`, produce `FramePacket`, send through channel |
| **CLEAR** | Clear change tracker. Game thread free for next frame |

### Render Thread

| Phase | Action |
|-------|--------|
| **RECEIVE** | Block on channel, receive `FramePacket` |
| **PREPARE** | Process `ResourceOp`s — upload meshes/textures, update materials, free resources |
| **CULL** | Frustum + occlusion culling. Produce sorted, batched draw list |
| **RECORD** | Render graph executes: each pass records GPU commands |
| **SUBMIT** | `queue.submit()` — command buffers sent to hardware |
| **PRESENT** | Swapchain present. Send `FrameFeedback`. Loop back to RECEIVE |

### Ownership Boundaries

| Data | Owner | Crosses boundary as |
|------|-------|---------------------|
| World, Components | Game Thread | Read-only in extract |
| FramePacket | Produced by Game, consumed by Render | Owned value through channel (Game → Render) |
| FrameFeedback | Produced by Render, consumed by Game | Owned value through channel (Render → Game, 2-frame lag) |
| GPU resources | Render Thread | Never — game code uses handles |
| Assets (CPU) | AssetServer (Game Thread) | Copied into `ResourceOp` at extract |
| Input | Main Thread | Snapshot borrowed by game tick |
| Audio commands | Collected on Game Thread | Drained by audio server each frame |
