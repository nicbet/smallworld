# SSE-driven traversal termination

## What

Add Screen-Space Error (SSE) computation to the raymarcher. When a voxel's projected screen footprint falls below a threshold (~1 px), skip the fine DDA inside the brick and shade from the brick's coarse representation. This is the "never do sub-pixel work" principle from DESIGN.md expressed as a traversal-stop rule.

## Why

At 1080p with 10cm voxels, full resolution is only needed within ~90m of the camera. Beyond that, individual voxels are sub-pixel — tracing them wastes GPU cycles for invisible detail. SSE termination makes rendering cost proportional to screen pixels, not world complexity.

## SSE formula

```
focal_length = screen_height / (2 * tan(fov_y / 2))
sse_pixels = voxel_world_size * focal_length / distance
```

When `sse_pixels < threshold` (default 1.0), the voxel is sub-pixel.

## Changes

### Uniform buffer

Add two fields to the Uniforms struct (Rust + WGSL):
- `focal_length: f32` — computed from `camera.fov_y` and render height
- `sse_threshold: f32` — LOD threshold in pixels (default 1.0)

These replace `_pad1` and `_pad2` in the existing padding.

### Shader: `trace_terrain` and `trace_object`

Before calling `trace_brick`, compute the distance from camera to the brick entry point and check SSE:

```wgsl
let dist = length(ro + rd * brick_t - u.camera_pos.xyz);
let sse = vs * u.focal_length / max(dist, 0.001);
if sse < u.sse_threshold {
    // Sub-pixel: shade from brick without fine DDA
    let color = sample_brick_lod(handle);
    let wp = ro + rd * brick_t;
    return HitResult(true, color, coarse_normal, vec3<i32>(0), handle, wp, brick_t);
}
```

### Shader: `sample_brick_lod`

New function that reads a representative color from a brick without tracing individual voxels. For this initial implementation (before mip chains land in sw-638630):
- Read the center voxel (8,8,8)
- If air, read (4,4,4), (12,12,12), (8,4,8)
- Use the first solid voxel's palette color
- If all sampled voxels are air, return no_hit (let the ray continue)

This is a cheap approximation — the mip chain story will replace it with properly filtered data.

### Raymarcher Rust side

- Compute `focal_length` from `camera.fov_y` and `self.height`
- Pack into uniforms, replacing two pad fields
- Add `sse_threshold` field to `Raymarcher` struct (default 1.0)

### Debug UI

Add an SSE threshold slider (0.5–4.0) to the debug panel so we can tune the cutoff visually and observe the quality/performance tradeoff.

## What this does NOT include

- Mip chain storage or filtering (sw-638630)
- Brick-level occupancy masks
- Per-brick average color precomputation

Those are follow-up stories. This story establishes the traversal-stop rule and coarse shading path.

## Acceptance Criteria

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] Default scene renders without visible artifacts at SSE threshold 1.0
- [ ] Distant terrain/objects shade from coarse brick color (visually confirmed)
- [ ] Debug panel slider adjusts SSE threshold in real time
- [ ] `--bench` with threshold 1.0 shows improved GPU compute time vs. baseline (run before/after)
- [ ] Raising threshold to 4.0 shows clear LOD banding (blocks become visible) — confirms the system is active
