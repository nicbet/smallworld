## What Was Built

Full deferred lighting pipeline: shadow mapping, clustered light grid, and a Cook-Torrance PBR shade pass. The engine now renders lit, shadowed, HDR scenes instead of flat albedo debug output.

## Architecture

The pipeline flow changed from `gbuffer → debug_blit → present` to:

```
gbuffer → shadow → shade (compute) → blit_hdr (Reinhard) → present
```

### New file: `lighting.rs`

Contains four subsystems in one module (~1200 lines):

**LightBuffer** — Packs up to 256 visible lights into a GPU SSBO every frame. Each `GpuLight` is 64 bytes: position/range, direction/type, color/intensity, spot params + shadow index. Directional lights use `range = -1.0` as sentinel. A separate `LightHeader` uniform carries the count.

**ShadowAtlas** — 4096×4096 `Depth32Float` texture (64 MB, matching Unity HDRP / Godot 4 defaults). Grid subdivision allocates regions for multiple shadow-casting lights (`sqrt(N)` subdivision). Owns a depth-only render pipeline that reuses the GBuffer vertex layout. Phase 1 renders one directional light with an orthographic projection fitted to the camera's forward frustum (30m × 30m, centered 10m ahead).

**ClusteredLightGrid** — CPU-side assignment of lights to screen-space clusters. 80px tiles × 24 logarithmic depth slices (Doom 2016 layout). Depth distribution uses `cluster_near = max(camera_near, 0.5)` to avoid the near-plane singularity. Directional lights are added to every cluster; point/spot lights use AABB-projected screen bounds + depth range. Output is three GPU buffers: offsets, counts, and packed light indices (max 32 per cluster).

**LightingPass** — Orchestrates everything. Creates the shade compute pipeline (4 bind groups: GBuffer+HDR, camera+lights, shadow atlas, cluster grid), dispatches at 8×8 workgroups, then blits the `Rgba16Float` HDR output to the swapchain surface with Reinhard tone mapping.

### New shaders

**`shadow.wgsl`** — Minimal depth-only vertex shader. Same vertex layout as `gbuffer.wgsl`, outputs only `@builtin(position)`.

**`shade.wgsl`** — Full-screen compute shader (~200 lines). Reads GBuffer via `textureLoad`, reconstructs world position from depth + `inv_view_proj`, looks up the cluster for each pixel, iterates that cluster's lights, evaluates Cook-Torrance BRDF (GGX distribution, Smith-GGX geometry, Schlick fresnel), applies shadow sampling and attenuation, writes to `Rgba16Float` storage texture. A minimal ambient term (`0.03 * albedo`) prevents pure-black shadows.

**`blit.wgsl`** — Updated with Reinhard tone mapping (`color / (color + 1)`).

### Modified files

**`gbuffer.rs`** — Debug albedo blit removed (pipeline, bind group layout, sampler). Added `gbuffer()` accessor so `LightingPass` can read GBuffer texture views. `render()` no longer takes `surface_view`. Added `front_face: wgpu::FrontFace::Cw` for correct mesh winding with wgpu's Y-flip.

**`engine.rs`** — `LightingPass` added as `Option<LightingPass>` (None for headless). Created alongside `GBufferPass`, resized alongside it. `render_frame` calls `gbuffer_pass.render()` then `lighting_pass.render()` within the same command encoder.

**`shaders.rs`** — `Shader::Shadow` and `Shader::Shade` variants added with `file_name()`, `baked()`, and test coverage.

**`sandbox/main.rs`** — Test scene expanded: 20×20m floor, 5 boxes with different materials (stone, metal, red plastic, wood), 3 light types (directional sun, warm point light, cool spot light). `make_box()` helper generates proper 24-vertex box meshes. Standard CCW winding (works with `FrontFace::Cw`).

## Key Decisions

**Compute over fragment for shade pass.** Research confirmed Unity HDRP uses compute, UE5 auto-switches to compute at >80 lights. Compute reads the GBuffer once per tile and writes directly to a storage texture — better bandwidth than fragment pass with color attachments.

**FrontFace::Cw convention.** wgpu's framebuffer Y points down, flipping the apparent winding of every triangle. Standard CCW meshes need `FrontFace::Cw` to be treated as front-facing. Without this, every mesh import (glTF, OBJ) would need its winding reversed. The floor quad's original index reversal (`[0,2,1]` instead of `[0,1,2]`) was a symptom of this — now fixed at the pipeline level.

**Standard front-face shadow rendering.** Initial implementation used back-face rendering (cull front faces) as a "clever" bias technique. This caused three rounds of debugging: peter panning, silhouette-edge acne, and contact artifacts. Reverted to the standard approach (cull back faces + rasterizer slope-scale bias), which every major engine uses and which worked immediately.

**4096² shadow atlas.** Sized to match Unity HDRP and Godot 4 defaults. Grid subdivision supports multiple shadow-casting lights from the start.

**Logarithmic cluster depth with clamped near.** `max(camera_near, 0.5)` avoids the first slices becoming infinitely thin. Hybrid lerp (lambda ~0.9) is a one-line upgrade if extreme depth ratios cause issues later.

**textureSampleCompare forbidden in compute.** wgpu/WGSL does not allow comparison sampling in compute shaders (fragment-only). Replaced with `textureLoad` + manual depth comparison. Loses hardware PCF filtering — tracked as wgpu friction item.

## Follow-ups Filed

- **sw-a6dea7** — GPU-side cluster assignment (E7)
- **sw-f967e8** — Multi-cascade CSM (E7)
- **sw-a08abf** — PCF / PCSS soft shadows (E7)
- **sw-0dd1c0** — Virtual shadow maps (E7)
- **sw-72e0e0** — GLB/glTF mesh import (E1.5)
