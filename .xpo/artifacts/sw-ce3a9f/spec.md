## What

Fullscreen compute-shader raymarcher that renders a dense 256³ voxel volume stored in a GPU storage buffer. A compute pass traces one ray per pixel via 3D-DDA, writes hit colors to a storage texture, and a fullscreen-triangle blit pass copies the result to the swapchain surface. The egui overlay renders on top.

This proves the complete wgpu toolchain — compute pipelines, storage buffers, storage textures, bind groups, uniform buffers, and multi-pass composition — before any sparse voxel structure exists.

## Why

M0 milestone gate: the foundation epic requires a dense brick rendering at interactive rates on Metal and Vulkan with measurement tooling on screen. This is the first real GPU work beyond the egui debug overlay.

## Acceptance Criteria

- A procedural test scene (ground plane + sphere) renders from a dense 256³ volume
- Free-fly camera movement updates the rendered view each frame
- egui debug overlay composites on top of the raymarched image
- `cargo clippy` and `cargo test` pass on all platforms
- Frame time is interactive (< 33 ms at 1280×720 on the development machine)

## Flow

### 1. Raymarch shader — `crates/engine/shaders/raymarch.wgsl`

New WGSL compute shader composed with `common.wgsl` via `shaders::compose`.

**Bindings (group 0):**
| Binding | Type | Contents |
|---------|------|----------|
| 0 | `uniform` | `Camera` struct: `inv_view_proj: mat4x4<f32>`, `camera_pos: vec4<f32>`, `resolution: vec2<f32>`, `_pad: vec2<f32>` |
| 1 | `storage, read` | `array<u32>` — 256³ voxel material IDs (0 = air) |
| 2 | `storage_texture, write, rgba8unorm` | Output image |

**Entry point:** `cs_main` at workgroup size `(8, 8, 1)`.

**Algorithm:**
1. Derive pixel coordinates from `global_invocation_id.xy`; discard if outside resolution
2. Compute clip-space position from pixel + resolution, transform through `inv_view_proj` to get world-space ray direction
3. Ray-AABB intersection against the volume bounds; miss → sky gradient based on ray direction y
4. 3D-DDA through the uniform grid: step one voxel at a time along the ray
5. On first non-zero voxel hit: determine face normal from the DDA step axis, shade with `max(0, dot(normal, sun_dir))` × material base color, store to output texture
6. Max step count safety cap (512) to prevent GPU hangs on edge cases

**Volume placement:** Centered at world origin. Min = `(-12.8, -12.8, -12.8)`, max = `(12.8, 12.8, 12.8)`. Constants in the shader — no uniform needed for this spike.

### 2. Blit shader — `crates/engine/shaders/blit.wgsl`

Fullscreen-triangle vertex + fragment shader. No vertex buffer — positions and UVs are hardcoded from `vertex_index`.

**Bindings (group 0):**
| Binding | Type | Contents |
|---------|------|----------|
| 0 | `texture_2d<f32>` | The compute output texture |
| 1 | `sampler` | Nearest-neighbor sampler |

**Vertex shader** emits a triangle covering `[-1, 1]` clip space with UVs `[0, 1]`.
**Fragment shader** samples the texture at the interpolated UV and outputs the color.

### 3. Shader enum — `crates/engine/src/shaders.rs`

Add `Shader::Raymarch` and `Shader::Blit` variants with corresponding `file_name()`, `baked()` match arms, and extend `ALL` in tests.

### 4. Raymarcher module — `crates/engine/src/raymarcher.rs`

New public module added to `lib.rs`. Struct `Raymarcher` owns all GPU resources:

```
pub struct Raymarcher {
    compute_pipeline: wgpu::ComputePipeline,
    blit_pipeline: wgpu::RenderPipeline,
    compute_bind_group: wgpu::BindGroup,
    blit_bind_group: wgpu::BindGroup,
    camera_buf: wgpu::Buffer,
    voxel_buf: wgpu::Buffer,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    width: u32,
    height: u32,
}
```

**Public API:**

- `Raymarcher::new(gpu: &GpuContext, width: u32, height: u32, surface_format: TextureFormat) -> Self`
  Creates all pipelines, allocates the 256³ storage buffer (fills with procedural test data), creates the output texture and bind groups.

- `resize(&mut self, gpu: &GpuContext, width: u32, height: u32)`
  Recreates the output texture and both bind groups at the new resolution.

- `render(&self, encoder: &mut CommandEncoder, surface_view: &TextureView, camera: &FreeCamera)`
  1. Writes camera uniforms to `camera_buf` via `queue.write_buffer`
  2. Dispatches the compute pass: `ceil(width/8) × ceil(height/8) × 1` workgroups
  3. Begins a render pass on `surface_view` (clear to black), draws the blit triangle, ends the pass

Wait — `render` needs the queue for `write_buffer`. Adjust signature to take `gpu: &GpuContext`.

- `render(&self, gpu: &GpuContext, encoder: &mut CommandEncoder, surface_view: &TextureView, camera: &FreeCamera)`

**Volume generation** (private helper called from `new`):
- Allocate `Vec<u32>` of length 256³
- Fill bottom 20 voxel layers (y < 20) with material 1 (ground)
- Fill a sphere of radius 60 voxels centered at (128, 128, 128) with material 2 (stone)
- Upload to storage buffer via `create_buffer_init` or `queue.write_buffer`

**Material palette** (in shader): material 1 → green-brown `(0.4, 0.55, 0.3)`, material 2 → warm gray `(0.7, 0.65, 0.6)`.

### 5. Viewer integration — `crates/viewer/src/main.rs`

- Add `raymarcher: Raymarcher` to `RunState`, constructed in `resumed()` after surface config
- In `Resized` handler: call `raymarcher.resize()`
- In `RedrawRequested`: call `raymarcher.render()` **before** the egui render pass
- Modify the egui render pass to use `LoadOp::Load` instead of `LoadOp::Clear` (the blit already wrote the background)

### 6. Debug panel

Add the output texture resolution to the debug panel (confirms compute dispatch covers the viewport).

## Decisions

**D1: Intermediate storage texture + blit pass, not direct surface write.**
Compute shaders cannot write to swapchain textures (no `STORAGE_BINDING` on surface textures). The blit pass also decouples compute output format from surface format. Cost: one extra fullscreen triangle draw (~free).

**D2: `Rgba8Unorm` for the intermediate texture, not `Rgba16Float`.**
8-bit precision loses quality in dark gradients, but this is a throwaway spike. Keeps memory low and avoids any feature negotiation.

**D3: `u32` per voxel (64 MB buffer), not packed `u8` (16 MB).**
WGSL storage buffers require 4-byte-aligned elements. Bit-packing 4 voxels per u32 adds shift/mask logic for no benefit in a spike that will be replaced by the brick pool.

**D4: No GPU timestamp queries in this issue.**
The design doc calls for GPU timings from day one, but timestamp queries are an adapter feature that needs negotiation and fallback paths. CPU-side frame timing (already present) is sufficient to validate interactive rates. GPU timestamps belong in a follow-up issue.

**D5: Volume centered at world origin as shader constants, not uniforms.**
Avoids adding fields to the camera uniform for a single hardcoded volume. The real system will have per-instance transforms.

**D6: Camera uniform via `queue.write_buffer` each frame, not a mapped staging buffer.**
Simpler, and 96 bytes per frame is negligible. The real system may use ring buffers later.

**D7: Single render pass for blit + egui.**
The blit pass clears and draws the fullscreen triangle; egui renders into the same pass. Avoids a redundant load/store cycle. This requires the blit render pipeline to target the same surface format as egui.

Actually — egui's `render()` takes a `&mut RenderPass` so the blit and egui can share one pass. The blit pipeline draws first (covering the whole screen), then egui draws on top with alpha blending.

## Edge Cases

**MEDIUM — Ray starts inside the volume.** The DDA entry `t` is clamped to `max(0, t_entry)`. If the camera is inside the volume, the march starts at t=0 (ray origin). The shader handles this naturally.

**LOW — Window resize to very small size.** Width/height are clamped to `max(1, ...)` in the existing resize handler. A 1×1 dispatch is fine.

**LOW — Zero-length ray direction.** Cannot happen: the inverse VP matrix is always invertible for a valid camera, and pixel centers never collapse to the camera position.

## Assumptions

- wgpu 30 supports `Rgba8Unorm` as a storage texture format on Metal and Vulkan without additional feature flags
- `max_storage_buffer_binding_size` (default 128 MB) is sufficient for the 64 MB voxel buffer
- `max_compute_workgroups_per_dimension` (default 65535) is sufficient for ceil(1280/8) = 160 workgroups
- The fullscreen blit triangle is effectively free compared to the compute dispatch
