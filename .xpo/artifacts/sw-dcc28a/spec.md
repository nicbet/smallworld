## What

Add shadow mapping, a clustered light grid, and a deferred shading pass to the Execute sub-pipeline. This replaces the debug albedo blit with actual lit output. The GBuffer (albedo, normals, material, depth) is already written by `GBufferPass` — this issue reads those textures and evaluates lighting.

## Why

Without lighting, the renderer is a flat-color debug view. Every downstream feature — tone mapping, SSAO, SSR, the game layer — needs lit HDR output to work against. This is the critical path to a usable renderer.

## How

### 1. LightBuffer — GPU light SSBO

**Struct layout (GPU side):**
```
struct GpuLight {
    position_range: vec4<f32>,    // xyz = position (point/spot), w = range
    direction_type: vec4<f32>,    // xyz = direction (dir/spot), w = type (0=dir, 1=point, 2=spot)
    color_intensity: vec4<f32>,   // xyz = color, w = intensity
    spot_params: vec4<f32>,       // x = inner_cos, y = outer_cos, z = shadow_index, w = unused
}
```

- `LightBuffer` struct on the Rust side owns a `wgpu::Buffer` (storage, dynamic size).
- Repacked every frame from `VisibilitySet::lights` — iterate `world.light(key)`, write into a mapped staging buffer, copy to GPU.
- A `light_count: u32` uniform accompanies the SSBO.
- Maximum 256 lights (buffer pre-allocated, count is the live subset).
- Directional lights use `position_range.w = -1.0` as sentinel (infinite range).

### 2. Shadow Atlas

**4096×4096 single atlas** — `Depth32Float`, 64 MB.

Sized to match Unity HDRP and Godot 4 defaults (see `notes/research/shadow-atlas-sizing.md`). URP uses 2048; 4096 gives us room for atlas subdivision without resizing.

**Atlas subdivision (in scope):**
- `ShadowAtlas` struct manages a simple grid of shadow map regions.
- Each shadow-casting light gets a `ShadowView` (view-proj matrix + atlas viewport rect).
- Initial allocation: power-of-two subdivision. One light at 4096, two lights at 2048 each, etc.
- `ShadowViews` array uploaded as a separate SSBO for the shade pass.
- Maximum 16 shadow views (enough for 1 directional + several point/spot shadows).

**Phase 1 shadow rendering:** one directional light, one shadow cascade.
- Shadow map region: largest available atlas slot.
- Light-space view-proj: orthographic projection fitted to the camera frustum (simple bounding sphere).
- Depth-only render pass reusing the GBuffer vertex shader (`vs_main`) with a shadow-specific pipeline (no color attachments, front-face culling for bias).
- Shadow bias: constant + slope-scale bias via `DepthBiasState`.
- `shadow_index` in `GpuLight.spot_params.z`: index into ShadowViews array. `-1.0` = no shadow.

**Shadow pipeline:** separate `RenderPipeline` with:
- Same vertex layout and vertex shader as GBuffer
- No fragment shader (depth-only)
- `DepthStencilState` with `Depth32Float`
- `cull_mode: Some(Face::Front)` (Peter Panning prevention)
- `depth_bias` for shadow acne prevention

### 3. Clustered Light Grid

**Grid dimensions:** Doom 2016-style — tile size 80px, 24 logarithmic depth slices.
- Tile count adapts to resolution: `ceil(width/80) × ceil(height/80) × 24`.
- At 1280×720: 16×9×24 = 3456 clusters.

**Depth distribution:** Logarithmic with clamped near-plane (see `notes/research/clustered-shading-depth.md`).
```
cluster_near = max(camera_near, 0.5)
Z_k = cluster_near * (far / cluster_near) ^ (k / N)
k = floor(N * log(z / cluster_near) / log(far / cluster_near))
```
Clamping near avoids the singularity (first slices becoming extremely thin) without hybrid complexity. If extreme depth ratios cause issues later, adding the hybrid `lerp` with `lambda ~0.9` is a one-line change (filed implicitly with sw-a6dea7 scope).

**CPU-side assignment (phase 1, GPU assignment deferred to sw-a6dea7):**
- Each frame, iterate visible lights, compute screen-space bounding volumes, assign to overlapping clusters.
- Directional lights added to every cluster.
- Point lights: sphere-cluster intersection.
- Spot lights: cone bounding sphere approximation.

**GPU buffer layout — offset + count indirection:**
```
cluster_offsets: Buffer<u32>  // [num_clusters] — offset into light_indices
cluster_counts: Buffer<u32>  // [num_clusters] — count per cluster
light_indices: Buffer<u32>   // packed light indices
```
Max 32 lights per cluster. Total worst-case ~1.3 MB at 1280×720.

### 4. Deferred Shade Pass

**Full-screen compute shader** (`shade.wgsl`).

Research confirms compute is the right choice for deferred: Unity HDRP uses compute (tile/cluster hybrid), UE5 auto-switches to compute at >80 lights, and compute wins on bandwidth for deferred (reads GBuffer once per tile, accumulates in shared memory). See `notes/research/shadow-atlas-sizing.md`.

- Workgroup: 8×8 threads, one thread per pixel.
- Dispatch: `ceil(width/8) × ceil(height/8)`.

**Inputs (bind group):**
- group 0: GBuffer textures (depth, albedo, normal, material) + sampler
- group 1: LightBuffer SSBO + light count uniform + camera uniforms (inv_view_proj, camera_pos, screen size)
- group 2: Shadow atlas texture + shadow sampler + shadow views SSBO
- group 3: Cluster grid buffers (offsets, counts, indices) + grid params uniform

**Camera uniforms for shade pass:**
```
struct ShadeUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,        // xyz = position, w = unused
    screen_size: vec4<f32>,       // xy = width/height, zw = 1/width, 1/height
    near_far: vec4<f32>,          // x = near, y = far, zw = unused
}
```

**Position reconstruction:** from depth + inv_view_proj (no position buffer — saves 33 MB per GBuffer spec).

**BRDF:** Cook-Torrance microfacet specular + Lambertian diffuse.
- D: GGX/Trowbridge-Reitz
- F: Schlick approximation
- G: Smith-GGX (height-correlated)
- Standard metallic workflow: `F0 = mix(0.04, albedo, metallic)`

**Shadow sampling:** single-tap hard shadows (PCF/PCSS deferred to sw-a08abf).

**Attenuation:**
- Point: `saturate(1.0 - (d/range)^4) ^ 2 / (d^2 + 1.0)` (UE4 smooth falloff)
- Spot: point attenuation × angular falloff `smoothstep(outer_cos, inner_cos, dot)`
- Directional: no distance attenuation

**Output:** `Rgba16Float` storage texture — HDR color.

### 5. HDR Preview (Reinhard)

Until tone mapping lands (sw-49724f), the blit pass applies simple Reinhard tone mapping:
```
color_out = hdr_color / (hdr_color + 1.0)
```
This gives a reasonable preview with bright lights — pure `saturate()` clips highlights and looks wrong. The blit shader gets a `tone_map` boolean uniform so we can disable it when proper tone mapping arrives.

### 6. Integration into render_frame

Current flow:
```
cull → stream → [gbuffer → debug_blit] → present
```

New flow:
```
cull → stream → [gbuffer → shadow → shade → blit_hdr] → present
```

- `GBufferPass::render` is split: GBuffer render stays, debug blit is replaced.
- New `LightingPass` struct owns: `LightBuffer`, `ShadowAtlas`, `ClusteredLightGrid`, shade pipeline, HDR output texture, HDR blit pipeline.
- `LightingPass::render()` called after `GBufferPass::render()`, receives GBuffer texture views + camera + world lights.
- `LightingPass` created alongside `GBufferPass` in `Engine::new()`, resized alongside it.

### 7. New files

| File | Purpose |
|------|---------|
| `crates/engine/src/lighting.rs` | `LightBuffer`, `ShadowAtlas`, `ClusteredLightGrid`, `LightingPass` |
| `crates/engine/shaders/shadow.wgsl` | Depth-only vertex shader for shadow pass |
| `crates/engine/shaders/shade.wgsl` | Deferred shade compute shader (PBR + shadows + clusters) |

### 8. Shader registration

Add `Shader::Shadow` and `Shader::Shade` to `shaders.rs` enum with corresponding `file_name()` and `baked()` entries.

## Acceptance Criteria

- [ ] `LightBuffer` packs visible lights into an SSBO every frame
- [ ] `ShadowAtlas` 4096² with grid subdivision supporting multiple shadow-casting lights
- [ ] Shadow pass renders depth from one directional light's perspective
- [ ] `ClusteredLightGrid` assigns lights to screen-space clusters on CPU (log depth, clamped near)
- [ ] Deferred shade compute pass outputs lit HDR color using Cook-Torrance BRDF
- [ ] Shadow sampling works for the directional light (single-tap hard shadows)
- [ ] Point and spot lights attenuate correctly (no shadows yet — only directional shadows in phase 1)
- [ ] HDR output blitted to swapchain with Reinhard tone mapping
- [ ] Sandbox scene shows lit floor with sun shadow and warm fill point light
- [ ] Zero new `unsafe` blocks
- [ ] All existing tests pass, new tests for LightBuffer packing and cluster assignment
- [ ] `cargo clippy` clean

## Follow-ups Filed

- **sw-a6dea7** — GPU-side clustered light assignment (optimization, depends on this issue)
- **sw-f967e8** — Multi-cascade CSM for directional shadows (depends on this issue)
- **sw-a08abf** — PCF / PCSS soft shadows (depends on this issue)

## Edge Cases

- **No lights in scene:** shade pass outputs black (ambient term = 0). Not an error.
- **Max lights exceeded (>256):** clamp, log a warning once per frame.
- **Shadow caster with no shadow flag:** skip shadow map render. `shadow_index = -1`.
- **Cluster overflow (>32 lights per cluster):** clamp, log once. Prioritize by intensity × inverse-distance.
- **Resize:** HDR texture recreated. Shadow atlas stays fixed. Cluster grid tile count recalculated. Shade dispatch size recalculated.
- **Headless engine:** `LightingPass` is `None` (same pattern as `GBufferPass`).
