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

_(OQ 5 resolution, 2026-08-11.)_ Bulk asset bytes never travel by value and are never memcpy'd on a hot thread. The engine owns a **staging pool**: CPU-visible mapped `wgpu` buffers, ring/size-class allocated, fence-reclaimed, and budgeted like every other pool (Principle 5 — wgpu's internal `write_*` staging would be invisible memory; ours is accounted).

- **Decode-direct population.** Asset IO/decode threads write decoder output _straight into_ a mapped staging region (rows 256-byte aligned at decode time; sequential writes — it's write-combined memory). This is the write the decoder performs anyway; no thread performs an additional payload copy.
- **O(1) render-thread cost.** The Render Thread records `copy_buffer_to_buffer` / `copy_buffer_to_texture` from staging into the device-local pools — command recording only, independent of payload size. (The alternative — `Arc` bytes + `queue.write_*` — would put an O(bytes) memcpy on the Render Thread per upload; rejected for steady-state streaming workloads.)
- **Firewall-clean.** Creating and mapping staging buffers off-thread is designed-for wgpu usage (`Device`/`Queue` are internally synchronized). This is engine-internal machinery; Principle 3 constrains game code, and the Render Thread's exclusive ownership of _device-local_ resources and submission is untouched.
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

This is the equivalent of UE5's `FMeshDrawCommand` — a fully stateless draw description that can be sorted, merged, and cached. Because the render side retains the mesh store across frames, static commands genuinely _are_ cached: sorted batch lists for static geometry are rebuilt only when the store changes, not per frame. Unlike UE5, we don't have the intermediate `FMeshBatch` layer as a data structure — its cross-backend role is played by the shared mesh stream itself; the extract step produces final draw commands directly.

#### LOD Transitions — the Fade/Dither Contract

_(OQ 10 resolution, 2026-08-11 — core mechanism; transition policy belongs to each backend.)_ Every pass that consumes the shared mesh stream (depth pre-pass, GBuffer, shadows) honors `InstanceData.fade` via **screen-door dithering**: a fragment is discarded when the dither threshold for its screen position exceeds `fade`, with the complement flag inverting the pattern. TAA resolves the stipple into a smooth cross-fade. A mesh LOD transition is therefore two temporary draws in the retained store — outgoing LOD fading out, incoming LOD fading in with the complement bit — upserted at transition start and collapsed to one when done (~150–300 ms window). Each screen pixel shows exactly one LOD at any instant, so depth and GBuffer stay consistent.

**The dither convention (pattern + fade→threshold mapping) is public contract, not an engine internal** — plugin-owned passes must be able to dither _complementary to_ shared-stream draws (the Voxel Plugin's tier handoff depends on this). Hard switches were tested and rejected (visible popping); geomorphing was tested and rejected (authoring-fragile, poor results unless perfect).

#### VolumeDrawCommand

The Voxel Plugin's custom-lane draw data — carried in its backend delta payload and consumed only by `VolumePass`. (Its extracted-mesh tiers travel the shared mesh stream as ordinary `MeshDrawCommand`s.)

```rust
struct VolumeDrawCommand {
    volume_id: EntityId,
    bounds:    AABB,
    lod_level: u8,        // demand hint: the LOD the game side wants — never residency
}
```

_(OQ 21, 2026-08-11: the former `brick_residency` field is gone. Residency truth lives with the brick pool on the streaming side — the game thread structurally cannot know it (feedback is ≥2 frames stale by design). Draw data carries demand only; `VolumePass` reads residency from the pool it renders from and falls back to coarser SVO parents for not-yet-resident bricks — the virtual-texturing residency pattern.)_

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

_(Defined as part of the OQ 11 resolution; carries the OQ 9 height-fog rider.)_

```rust
struct EnvironmentParams {
    sky:        SkyMode,
    ambient:    AmbientMode,
    height_fog: HeightFogParams,
    wind:       WindParams,      // direction, strength, gustiness — foliage vertex animation (OQ 29)
}

enum SkyMode {
    Procedural { turbidity: f32, ground_albedo: Vec3 },  // Hillaire LUT atmosphere (OQ 31), sun-driven
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

_(OQ 2 resolution, 2026-08-11.)_ The implementation of every "public input slot" in this document — GI, per-light shadow masks, sky visibility, and any future slot: **always-bound neutral dummies + per-frame uniform flags**. Never optional bindings (WGSL has none), never pipeline permutations as architecture.

- **Declaration.** A consuming pass declares each optional slot with a name, format, and neutral value. The graph binds the producer's output when one is registered, or a 1×1 dummy holding the neutral value when none is (gi = 0, shadow mask = 1, sky visibility = 1) — so even a flag bug degrades to the fallback look, never to garbage.
- **Per-frame flags.** A uniform bitfield tells the shader which slots are live this frame. Shaders branch on it — uniform control flow, coherent across the whole dispatch, effectively free on modern GPUs. A producer that skips a frame (e.g., RT budget throttling) clears its flag with zero bind-group churn; bind groups rebuild only when producers register or unregister.
- **One pipeline per consumer.** No shader-variant system, no PSO explosion; runtime feature toggles are a flag write. Pipeline permutations remain available as a _targeted optimization_ (e.g., a dedicated no-RT lighting variant for low-end hardware) — the graph knows producer presence at build time, so promoting a proven-hot variant is cheap. Trigger: profiling shows a register-pressure/occupancy win. Permutation as optimization, never as architecture.
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

Translucent voxel _media_ (smoke, fire) is handled separately from solid volumes: far-field media injects into the froxel grid via the public injector contract; near-field hero media renders in a plugin-owned raymarch pass in the transparency stage (see Volumetrics and Transparency). The opaque `VolumePass` never renders media.

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
