# Walkthrough: Brick mip chain

## What changed

Added a 4-level intra-brick mip chain that provides pre-averaged RGBA data for the SSE traversal fallback. This replaces the probe-based `sample_brick_lod` with spatially-aware, properly filtered mip lookups, making LOD transitions seamless.

Also renamed `trace_terrain` → `trace_grid` and `trace_object` → `trace_instance` throughout the shader to reflect that the engine provides generic dense-grid and instanced-volume traversal, not terrain-specific or object-specific logic.

## New: `crates/engine/src/mip.rs`

### Data layout

585 u32 words per brick, 4 levels:

| Level | Edge | Count | Offset | Covers |
|---|---|---|---|---|
| 1 | 8 | 512 | 0 | 2³ voxels |
| 2 | 4 | 64 | 512 | 4³ voxels |
| 3 | 2 | 8 | 576 | 8³ voxels |
| 4 | 1 | 1 | 584 | full 16³ brick |

Each mip voxel is packed RGBA: `R | (G << 8) | (B << 16) | (occupancy << 24)`.

### Averaging rules

- **Level 1**: walks 2×2×2 blocks of the 16³ source voxels, resolves each via the palette, averages non-air RGB, computes occupancy = `solid_count * 255 / 8`.
- **Levels 2–4**: recursively filter from the previous level. RGB averages only non-zero-alpha children (air doesn't dilute color). Occupancy averages ALL children including air (so a 50% solid brick gets ~127 alpha, not 255).

### Tests

- `all_air_produces_zero_mips` — empty bricks produce all-zero mip data
- `fully_solid_produces_nonzero_at_all_levels` — solid bricks propagate exact color to top mip
- `half_solid_has_correct_occupancy` — bottom-half-solid brick produces ~50% occupancy at top mip

## Modified: `crates/engine/src/brick_pool.rs`

Added `mip_buf: wgpu::Buffer` (585 words × capacity × 4 bytes), `write_mips(queue, handle, data)`, and `mip_buffer()` accessor. Log line updated to report mip buffer size.

## Modified: `crates/engine/shaders/raymarch.wgsl`

### New binding and function

`@group(0) @binding(8) var<storage, read> mips: array<u32>;`

`sample_brick_mip(handle, local_uv, lod)` — selects the mip level's offset and edge size from const arrays, maps `local_uv` to a mip-space coordinate, reads one packed u32, unpacks to vec4<f32>.

### Updated SSE fallback (both `trace_grid` and `trace_instance`)

```
let ratio = threshold / sse;
let lod = clamp(ceil(log2(ratio)), 1, 4);
let entry = (ray_at_brick_t - brick_min) / brick_size;
let mip_color = sample_brick_mip(handle, entry, lod);
if mip_color.a > 0 → return hit with mip_color.rgb
```

Graduated LOD: closer bricks get finer mip levels (more spatial resolution), distant bricks get coarser levels. The ray entry point selects the spatially-appropriate mip voxel at each level.

### Rename

`trace_terrain` → `trace_grid`, `trace_object` → `trace_instance`, `terrain_vs` → `grid_vs`. Comments updated throughout. The engine provides generic dense-grid and instanced-volume traversal; what goes in them is the game's concern.

## Key decisions

- **Separate mip buffer** rather than appending to voxel buffer — keeps the existing `handle * WORDS_PER_BRICK` stride clean and avoids changing every voxel read.
- **Alpha = occupancy over all children** (including air), not just solid children — ensures a half-solid brick shows ~50% occupancy, enabling correct LOD behavior.
- **Default SSE threshold 0.8** — below this, brick-boundary grid artifacts are invisible; above 1.0, the per-brick averaging creates visible seams between adjacent bricks with different material mixes.

## Known limitation

At aggressive SSE thresholds (> 0.8), adjacent bricks' independently averaged colors create a visible grid pattern at distance. The fix is cross-brick mip blending (reading neighbor handles in the shader) or a hierarchical mip structure above the brick level. Both are out of scope for this story.
