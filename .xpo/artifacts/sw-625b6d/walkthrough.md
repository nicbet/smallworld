
# Direct Coarse Worldgen — Walkthrough

## What was built

A GPU compute shader (`worldgen_coarse.wgsl`) that generates mip levels 2–4 directly from noise evaluation at sub-block centers, without generating full 16³ voxel data first. This is the first step toward coarse-to-fine generation for km-scale worlds.

## How it works

### Shader: `worldgen_coarse.wgsl`

73 threads per workgroup, one per coarse mip entry:
- Threads 0–63: level 2 (4³ sub-blocks), evaluate noise at sub-block center
- Threads 64–71: level 3 (2³ sub-blocks)
- Thread 72: level 4 (1³, brick center)

Each thread evaluates the full density function (terrain noise + cave noise) at its position and packs the result as RGBA u32 (same format as existing mip data). Material selection uses the same density thresholds as the full gen. Alpha derived from density magnitude.

### Key decision: caves must be included

Initial implementation skipped cave noise for coarse gen ("sub-brick detail"). This was wrong — caves are multi-brick voids. Skipping them caused solid coarse mip blocks where caves should be, creating visible artifacts where the coarse path showed filled terrain but the full-res path showed cave openings.

### Rust: `GpuWorldGenerator`

Two new methods:
- `generate_coarse(dims, device, queue, coarse)` — dispatches coarse shader in batches of 256, reads back directly to `CoarseMipGrid::write_cell_raw()`
- `generate_near(center, radius, dims, device, queue)` — scoped full 16³ generation for nearby bricks only, populating the GPU cache for the pager

### Preset separation

Small presets (Default, TerrainOnly) use full gen for everything — they preload_all so coarse gen is unnecessary. Large World uses coarse gen for the entire grid + near gen for the preload radius.
