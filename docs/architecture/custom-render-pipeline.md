## Customizing the Render Pipeline

Smallworld's rendering architecture is modular at five levels, mirroring UE5's customization depth but expressed in Rust traits rather than C++ inheritance. The guiding principle: **the engine's own voxel volume support ships as the Voxel Plugin — a geometry backend plugin, not a special case.** It uses the same backend traits a game would use to add GPU particles, procedural terrain, or SDF shapes. If the API isn't powerful enough for voxels, it isn't powerful enough for games.

### 1. Custom Geometry Types (`GeometryExtractor` + `GeometryRenderer`)

The deepest customization point. A geometry backend defines a new kind of renderable — its game-side component, how it extracts into scene deltas, how the render side retains and culls that data, which GPU resources it needs, and which render passes it contributes.

A backend is **two objects, one per side of the thread firewall** — the same split as UE5's `UPrimitiveComponent` / `FPrimitiveSceneProxy`. The extractor lives on the Game Thread and never sees GPU types. The renderer lives on the Render Thread, owns its retained scene data, and may use wgpu directly — it _is_ render-side code; Principle 3 constrains game code, not render plugins.

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

The shared mesh stream is smallworld's equivalent of UE5's `FMeshBatch` common currency: it is _why_ custom geometry gets shadows, occlusion, and RT presence for free. A backend that can express its geometry as triangles — even coarsely — should. The Voxel Plugin's extracted-mesh LOD tiers flow through this lane; only its raymarched near-field detail needs the custom lane.

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

| Backend                        | Component        | Shared mesh stream       | Custom lane                         | Own passes                                                                                                  | Shadow / RT participation                                              |
| ------------------------------ | ---------------- | ------------------------ | ----------------------------------- | ----------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| `MeshBackend`                  | `MeshRenderer`   | All draws                | —                                   | — (engine passes consume the shared store: `DepthPrepass`, `GBufferPass`, `ShadowPass`, `TransparencyPass`) | Via shared stream                                                      |
| Voxel Plugin (`VolumeBackend`) | `VolumeRenderer` | Extracted-mesh LOD tiers | Brick/SVO residency + raymarch data | `VolumePass`                                                                                                | Shared stream (extracted tiers) + `ShadowCaster` for raymarched detail |

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

The engine composes the final shader by concatenating the standard GBuffer output code with the custom fragment. Custom materials control _how_ the GBuffer inputs are computed and _which lighting response_ consumes them — not where they go. Lighting behavior beyond the registered shading models requires a custom pass (level 3/4).
