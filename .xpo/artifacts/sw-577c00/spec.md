## What

First sub-stages of Execute. Render visible cached meshes from the Stream stage into a GBuffer with depth. Build the HZB mip chain from depth for next frame's occlusion culling. Blit albedo to the swapchain as a debug view to validate the full pipeline end-to-end. Replaces the placeholder cube renderer.

## Why

This is where data becomes pixels for the first time. The pipeline so far flows World → Cull → Stream, producing `StreamOutput` with GPU mesh references. Execute consumes those meshes and renders them. The GBuffer is the convergence point for both voxel and mesh paths — downstream lighting (sw-dcc28a) reads it without knowing what produced the fragments.

The HZB feeds back into next frame's Cull stage for GPU occlusion culling.

## Acceptance Criteria

### GBuffer
- `GBuffer` struct owns three textures + depth:
  - Depth: Depth32Float (world-space position reconstructed from depth + inverse VP in lighting shader)
  - Albedo: Rgba8UnormSrgb (base color RGBA)
  - Normal: Rgba8Unorm (octahedral-encoded world-space normal in RG, BA spare)
  - Material: Rgba8Unorm (R=roughness, G=metallic, BA spare)
- Created at surface dimensions, recreated on resize
- Single render pass with depth + 3 color attachments (MRT)

### GBuffer shader
- WGSL vertex shader: transforms mesh vertices by model + view_proj matrices
- WGSL fragment shader: writes albedo (from material base_color), octahedral-encoded normal, roughness/metallic
- Vertex buffer layout matches `mesh::Vertex` (position, normal, UV, tangent)
- Octahedral normal encoding: world-space normal → 2-channel octahedral mapping (standard, used by UE5)

### HZB builder
- Compute shader: downsamples depth into a mip chain (power-of-two, max of each 2x2 block)
- Output stored for next frame's Cull stage (passed as `Some(&hzb_view)`)

### Debug blit
- Blit albedo GBuffer target to swapchain surface (reuse existing `blit.wgsl` pattern)
- Validates the full pipeline visually — flat unlit albedo colors

### Integration
- `GBufferPass` struct replaces `PlaceholderRenderer` in Engine
- Consumes `StreamOutput` — draws volume meshes and mesh instance meshes
- Per-mesh-instance: compute model matrix from position/rotation/scale, look up material for base_color
- HZB texture view passed to next frame's `cull_stage.cull(..., Some(&hzb_view))`
- Sandbox test scene (floor quad + volumes) visible as flat albedo

### Cleanup
- Remove `PlaceholderRenderer` and `placeholder.wgsl`
- Remove `placeholder` module from `lib.rs`

### General
- `cargo test` and `cargo clippy` pass
- Sandbox renders the test scene as flat albedo colors

## Flow

### 1. New shader `crates/engine/shaders/gbuffer.wgsl`

```wgsl
struct FrameUniforms {
    view_proj: mat4x4<f32>,
}

struct DrawUniforms {
    model: mat4x4<f32>,
    base_color: vec4<f32>,
    roughness: f32,
    metallic: f32,
}

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
}

struct GBufferOutput {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,   // octahedral RG, spare BA
    @location(2) material: vec4<f32>, // roughness, metallic, 0, 0
}
```

**Octahedral encoding** (in fragment shader):
```wgsl
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let n_abs = abs(n);
    var p = n.xy / (n_abs.x + n_abs.y + n_abs.z);
    if (n.z < 0.0) {
        p = (1.0 - abs(p.yx)) * sign_not_zero(p);
    }
    return p * 0.5 + 0.5; // map [-1,1] to [0,1] for Unorm storage
}
```

Decoding (in future lighting shader):
```wgsl
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let e2 = e * 2.0 - 1.0;
    let n = vec3(e2, 1.0 - abs(e2.x) - abs(e2.y));
    if (n.z < 0.0) { n = vec3((1.0 - abs(n.yx)) * sign_not_zero(n.xy), n.z); }
    return normalize(n);
}
```

### 2. New shader `crates/engine/shaders/hzb.wgsl`

Compute shader. Each thread reads a 2x2 block from the source mip, takes the max depth (farthest = most conservative for occlusion), writes to the next mip level. Dispatched once per mip level.

### 3. New file `crates/engine/src/gbuffer.rs`

**GBuffer struct:**
- Owns depth texture + 3 color textures + their views
- `new(device, width, height)` creates all textures
- `resize(device, width, height)` recreates if dimensions changed
- Total: 8 bytes/pixel (albedo 4 + normal 4 + material 4) + depth 4 = 16 bytes/pixel at 1080p ≈ 33 MB (half the cost of explicit position + Rgba16Float normals)

**GBufferPass struct:**
- Owns the render pipeline (MRT), bind group layout, uniform buffer
- Owns the HZB builder (compute pipeline + mip chain texture)
- Owns the debug blit pipeline (reuses blit.wgsl fullscreen triangle)
- `render(device, queue, encoder, surface_view, camera, world, stream_output)`:
  1. GBuffer pass: for each mesh in stream_output, set model matrix + material uniforms, draw
  2. HZB build: dispatch compute shader over depth mip chain
  3. Debug blit: sample albedo, write to swapchain surface

**Per-draw uniform writes:**
- Per-frame: write view_proj once
- Per-draw: write model matrix + material properties before each draw call
- Acceptable for skeleton scene size (a few objects). Instanced drawing tracked in sw-117099.

### 4. Shader registration — `crates/engine/src/shaders.rs`

Add `GBuffer` and `Hzb` variants to the `Shader` enum with corresponding files.

### 5. Engine integration — `crates/engine/src/engine.rs`

- Replace `PlaceholderRenderer` field with `GBufferPass`
- `render_frame`:
  1. drain_changes
  2. cull(world, view, hzb_from_last_frame)
  3. stream(world, visibility, ...)
  4. gbuffer_pass.render(... stream_output ...)
  5. Store HZB view for next frame's cull

### 6. Remove placeholder

- Delete `crates/engine/src/placeholder.rs`
- Delete `crates/engine/shaders/placeholder.wgsl`
- Remove `pub mod placeholder;` from `lib.rs`

### 7. Resize handling

- `Engine::resize` calls `gbuffer_pass.resize(...)` instead of `placeholder.resize(...)`

## Decisions

- **No explicit position buffer** — reconstruct world-space position from depth + inverse view-projection in the lighting shader. One matrix multiply per pixel, eliminates a 33 MB Rgba32Float target. Every production engine does this.
- **Octahedral normal encoding in Rgba8Unorm** — 2 channels encode a unit normal with < 1° max error. Standard technique (UE5, Godot). Halves normal storage vs Rgba16Float. Encoding/decoding is a few ALU ops.
- **No separate depth prepass** — the GBuffer fragment shader writes constants (no texture sampling, no complex math). There's zero overdraw cost to save. A depth prepass adds a full geometry pass for no benefit. Add one when fragment shaders become expensive (texture materials, complex BRDF).
- **Single fused GBuffer pass** — depth + all color targets in one render pass. Maximizes tile-based GPU efficiency (mobile, Apple Silicon). All data stays in tile memory.
- **Per-draw uniform writes** — write uniform buffer before each draw call. Simpler than dynamic offsets for the skeleton. Instanced/indirect drawing tracked in sw-117099.
- **Debug blit to albedo** — validates pipeline end-to-end without lighting. Removed when sw-dcc28a takes over.
- **HZB max reduction** — max of each 2x2 block (farthest depth). Conservative for occlusion: anything behind the farthest depth in a tile is definitely occluded.
- **GBuffer total: 16 bytes/pixel** — albedo (4) + normal (4) + material (4) + depth (4). At 1080p ≈ 33 MB. Efficient for the information density.

## Edge Cases

- **Empty StreamOutput**: GBuffer pass clears targets, blit shows clear color. (LOW)
- **Resize during frame**: GBuffer + HZB textures recreated. (LOW)
- **HZB mip chain for non-power-of-two surfaces**: round up to next power of two for the mip chain, clamp texture reads at edges. (MEDIUM — handle in compute shader with bounds check)

## Assumptions

- `StreamOutput` lifetime is within the same `render_frame` scope — guaranteed.
- Existing `blit.wgsl` fullscreen triangle pattern reusable for debug blit.
- wgpu MRT (3 color attachments + depth) is universally supported.
