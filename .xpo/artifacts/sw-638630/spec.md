# Brick mip chain build + re-filter

## What

Add 4 intra-brick mip levels (8³, 4³, 2³, 1³) storing pre-averaged RGBA per mip voxel. The SSE fallback reads from the appropriate mip level based on distance, using the ray's entry point to select the spatially correct mip voxel. This replaces the current `sample_brick_lod` probe hack with properly filtered data.

## Why

The current SSE fallback samples a single probe voxel, producing color mismatches at the LOD boundary (e.g., stone color on a grass-dominated brick). Properly filtered mip colors make the LOD transition seamless — the coarse representation visually matches the fine detail.

## Design

### Mip data format

Each mip voxel is a packed `u32`: `R | (G << 8) | (B << 16) | (occupancy << 24)`. RGB is the average of non-air children's resolved palette colors. Occupancy is `solid_count * 255 / child_count`. Air children don't dilute the color.

### Storage: separate GPU buffer

585 u32 words per brick across 4 levels:

| Level | Edge | Voxels | Offset | Covers |
|---|---|---|---|---|
| 1 | 8 | 512 | 0 | 2³ original voxels |
| 2 | 4 | 64 | 512 | 4³ |
| 3 | 2 | 8 | 576 | 8³ |
| 4 | 1 | 1 | 584 | entire 16³ brick |

At 32K capacity: 76.7 MB. Existing voxel+palette is ~192 MB, so mips add ~40%.

### Mip computation (`mip.rs`)

`compute_brick_mips(voxels: &[u8; 4096], palette: &[[u8; 4]]) -> [u32; 585]`

Level 1: walk 2³ blocks of the 16³ source, resolve each voxel via palette, average non-air RGB, compute occupancy fraction. Levels 2–4: recursively filter from the previous level's RGBA data.

### Shader lookup

Replace `sample_brick_lod` with `sample_brick_mip(handle, local_uv, lod)`:
1. Compute `lod = clamp(ceil(log2(threshold / sse)), 1, 4)` — graduated quality
2. Map ray entry point to mip-space coordinates: `floor(local_uv * mip_edge)`
3. Read one u32 from the mip buffer, unpack RGBA
4. If occupancy > 0, return hit with the averaged color. Otherwise fall through.

### Distance bands (1080p, 45° FOV, threshold 0.5)

| Distance | LOD | Mip edge | Effective resolution |
|---|---|---|---|
| 0–90m | 0 | 16 (full DDA) | Full voxel detail |
| 90–180m | 1 | 8 | 2-voxel blocks |
| 180–360m | 2 | 4 | 4-voxel blocks |
| 360–720m | 3 | 2 | 8-voxel blocks |
| >720m | 4 | 1 | Whole brick |

## Flow

1. Create `crates/engine/src/mip.rs` — `compute_brick_mips()`, `MIP_WORDS_PER_BRICK` constant
2. Add `mip_buf` to `BrickPool` — `write_mips()`, `mip_buffer()` accessor
3. Wire mip computation into brick creation paths (terrain in `scenes.rs`, objects in `voxel_object.rs`)
4. Add binding 8 to raymarcher bind group layout + bind group creation + resize
5. Add `MIP_WORDS_PER_BRICK` to `common.wgsl`
6. Replace `sample_brick_lod` with `sample_brick_mip` in `raymarch.wgsl`
7. Update SSE fallback in both `trace_terrain` and `trace_object` to use graduated LOD

## Acceptance Criteria

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] Default scene: LOD transition is visually smooth (no color popping)
- [ ] SSE slider at 2.0 shows graduated quality bands, not a hard line
- [ ] SingleBrick preset: mip data visible when zooming out (brick becomes a single colored block)
- [ ] All 6 presets load without crash (mip buffer correctly sized for each pool capacity)
