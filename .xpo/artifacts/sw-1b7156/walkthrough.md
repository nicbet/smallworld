# Walkthrough: SSE-driven traversal termination

## What changed

Added Screen-Space Error (SSE) computation to the raymarcher. When a voxel's projected screen footprint falls below a configurable threshold, the fine DDA inside a brick is skipped and the brick is shaded from a sampled representative color.

## Engine changes

### `crates/engine/src/raymarcher.rs`

- Added `focal_length` and `sse_threshold` fields to the `Uniforms` struct, replacing two padding fields (no size change)
- `focal_length` computed per frame as `render_height / (2 * tan(fov_y / 2))` — this converts world-space sizes to screen pixels at a given distance
- `render()` takes a new `sse_threshold: f32` parameter

### `crates/engine/shaders/raymarch.wgsl`

- Uniform struct mirrors the Rust side: `focal_length` and `sse_threshold` fields
- New `sample_brick_lod(handle)` function: probes 4 positions inside a brick (center, then 3 fallbacks) and returns the first solid voxel's palette color. Returns `vec3(-1)` if all probes are air, so the ray continues rather than creating a false hit on an empty brick.
- SSE check added in both `trace_terrain` and `trace_object`, before calling `trace_brick`:
  ```
  let sse = voxel_size * focal_length / distance;
  if sse < sse_threshold { ... shade from sample_brick_lod ... }
  ```
- For objects, the distance is converted to world space via `rd_scale` before the SSE comparison

## Sandbox changes

### `crates/sandbox/src/main.rs`

- `sse_threshold: f32` added to `RunState` (default 0.5)
- Egui slider (0.0–4.0, step 0.1) labeled "SSE" in the debug panel
- Threshold passed through to `Raymarcher::render()`

## Design decisions

- **Distance metric**: uses ray t-parameter (distance along ray) rather than Euclidean camera-to-brick distance — cheaper and directionally correct for perspective projection
- **Default threshold 0.5**: conservative — only activates when voxels are truly sub-half-pixel. At 1080p with 10cm voxels this is ~180m+ from camera. The slider allows tuning up for performance or down for quality.
- **Dithered LOD transition attempted and reverted**: spatial noise on the SSE threshold created visible point-cloud artifacts because the coarse brick color (a single sampled voxel) doesn't match the fine detail. Proper smooth transitions require the mip chain (sw-638630) to provide filtered average colors that visually match.
- **sample_brick_lod is a temporary placeholder**: probes 4 fixed positions inside the brick. The mip chain story will replace this with properly filtered data, making the LOD transition seamless.

## What this enables

At km scale (future stories), SSE termination means rendering cost is proportional to screen resolution, not world complexity. Full-res voxel tracing only happens in a ~100–180m bubble around the camera. Everything beyond uses the coarse path, which will look correct once mip chains provide filtered colors.
