## What was built

A fullscreen compute-shader raymarcher that renders a dense 256³ voxel volume at interactive rates. This is the first real GPU work in the engine — it proves the complete wgpu toolchain (compute pipelines, storage buffers, storage textures, bind groups, uniform buffers, multi-pass composition) before any sparse voxel structure is introduced.

## Architecture

The rendering pipeline has three passes per frame, all recorded into a single command encoder:

1. **Compute pass** (`raymarch.wgsl`) — dispatches `ceil(width/8) × ceil(height/8)` workgroups of 8×8 threads. Each thread traces one ray through the 256³ volume and writes the shaded color to an `Rgba8Unorm` storage texture.

2. **Blit pass** (`blit.wgsl`) — a fullscreen triangle (3 vertices, no vertex buffer, positions derived from `vertex_index`) samples the storage texture and writes to the swapchain surface. This pass exists because compute shaders cannot write directly to swapchain textures (they lack `STORAGE_BINDING`).

3. **egui pass** (unchanged) — renders the debug overlay on top using `LoadOp::Load` (previously `LoadOp::Clear`, since the blit now provides the background).

## How the pieces fit together

### Ray generation (compute shader)

Each thread computes a clip-space coordinate from its pixel position and the camera resolution uniform. Two points (at NDC z=0 and z=1) are transformed through the inverse view-projection matrix to get world-space near and far points. The ray direction is `normalize(far - near)`, and the origin is the camera position. This approach works with any projection matrix without needing to decompose FOV/aspect.

### DDA traversal

The shader uses Amanatides & Woo 3D-DDA:

1. Ray-AABB test against the volume bounds (centered at world origin, ±12.8m). Entry `t` is clamped to `max(0, t_entry)` so rays starting inside the volume work correctly.
2. The entry point is converted to voxel coordinates (divide by `VOXEL_SCALE`, floor to integer).
3. Standard DDA stepping: maintain `t_max` (distance to next voxel boundary) per axis, always step along the axis with smallest `t_max`. Track which axis was crossed to derive the face normal.
4. On first non-zero voxel hit: shade with `dot(face_normal, sun_direction)` scaled between an ambient floor (0.25) and full diffuse. A 512-step safety cap prevents GPU hangs.
5. On miss (exited volume or hit step cap): output a sky gradient based on ray direction Y.

### `Raymarcher` struct (`crates/engine/src/raymarcher.rs`)

Owns all GPU resources for the raymarcher:

| Resource | Purpose | Size |
|----------|---------|------|
| `camera_buf` | Uniform buffer: inverse VP matrix, camera position, resolution | 96 bytes |
| `voxel_buf` | Storage buffer: 256³ × u32 material IDs | 64 MB |
| `output_texture` | Storage texture: `Rgba8Unorm`, screen resolution | ~14 MB at 2560×1440 |
| `compute_pipeline` | Compute pipeline for `raymarch.wgsl` | — |
| `blit_pipeline` | Render pipeline for `blit.wgsl` | — |
| Two bind group layouts + bind groups | Connecting resources to shaders | — |
| `sampler` | Nearest-neighbor sampler for the blit | — |

**Public API:**
- `new(gpu, width, height, surface_format)` — creates everything, generates the test volume, uploads to GPU
- `resize(gpu, width, height)` — recreates the output texture and both bind groups (pipelines are resolution-independent)
- `render(gpu, encoder, surface_view, camera)` — writes camera uniforms, dispatches compute, draws blit

Bind groups are rebuilt on resize because the output texture view changes and wgpu bind groups are immutable.

### Shader composition

`raymarch.wgsl` is composed with `common.wgsl` via `shaders::compose(&[Shader::Common, Shader::Raymarch])`. This gives the compute shader access to `VOXEL_SCALE` and `BRICK_EDGE` constants. `blit.wgsl` is standalone (no shared constants needed).

### Test volume

Generated on the CPU at startup (runs once, ~16M iterations for 256³):
- **Ground** (material 1, green-brown): voxels with y < 80 — a flat plane occupying the bottom third of the volume
- **Sphere** (material 2, warm gray): radius 40 voxels centered at (128, 128, 128) — sphere takes priority, carving into the ground where they overlap

The volume is centered at world origin. The default camera at (0, 2, 5) is outside the sphere (distance ≈54 > radius 40) and inside the volume, giving a view of the sphere with ground below and sky above.

## Key decisions

- **u32 per voxel** (64 MB total) rather than packed u8 (16 MB). WGSL storage buffers require 4-byte-aligned types, so packing would add bit-manipulation in the shader for no benefit in a throwaway spike. The default `max_storage_buffer_binding_size` (128 MB) accommodates this.

- **Camera uniforms via `queue.write_buffer`** each frame. At 96 bytes/frame this is negligible. A mapped staging ring buffer would save a GPU-side copy but adds complexity inappropriate for a spike.

- **Separate blit pass** rather than `copy_texture_to_texture`. The blit pass decouples the compute output format (`Rgba8Unorm`) from the surface format (sRGB), and the fullscreen triangle is effectively free compared to the compute dispatch.

- **No GPU timestamp queries** in this issue. The adapter feature needs negotiation and fallback paths — CPU-side frame timing (already in the debug panel) is sufficient to validate interactive rates for now.

## What a future reader should know

- The DDA traversal is intentionally simple and throwaway. The real engine will use hierarchical DDA with mip-level early termination (DESIGN.md §4). The ray generation and pipeline plumbing, however, are production patterns.

- The `CameraUniforms` struct uses `bytemuck` for zero-copy GPU upload. The layout matches WGSL's uniform buffer alignment rules: `mat4x4` at offset 0 (64 bytes), `vec4` at offset 64 (16 bytes), `vec2` at offset 80 (8 bytes), 8 bytes padding to reach 96 (multiple of 16).

- The blit's fullscreen triangle uses the `vertex_index` trick: three vertices at (-1,-1), (3,-1), (-1,3) cover the entire [-1,1] clip space with a single triangle. No vertex buffer needed.

- wgpu 30 changed `PipelineLayoutDescriptor`: `push_constant_ranges` was replaced by `immediate_size`, and `bind_group_layouts` entries are now `Option<&BindGroupLayout>`.
