
# Large World + Persistent Coarse Mip Grid — Walkthrough

## What was built

A Large World sandbox preset (128×16×128, 262K cells) that exercises the paging system at scale, backed by a persistent coarse mip grid that renders distant terrain at low resolution instead of showing empty space.

## Key architectural decisions

### Persistent coarse mip grid (`coarse_mip_grid.rs`)

A grid-parallel GPU buffer storing mip levels 2–4 (73 u32 = 292 bytes per cell). Written once when a brick is first generated, never cleared on eviction. The shader falls back to this data when the full brick has been evicted.

At 262K cells: 73 MB — fits comfortably in a single storage buffer binding.

The shader checks: if `grid_map[cell] != EMPTY` → full brick path (existing). Else → check `coarse_mips[cell * 73 + 72]` (level 4 top mip). If non-zero, sample at the appropriate LOD level (2/3/4 by SSE).

### Adapter limits fix (`gpu.rs`)

The biggest unlock: requesting `adapter.limits()` instead of `wgpu::Limits::default()`. The M1 Max supports 4 GB storage buffer bindings — wgpu's default was 128 MB. This allowed increasing the pool from 32K to 131K bricks (~1 GB total GPU memory).

### Pager performance improvements

1. **Worker-side mip computation** — `compute_brick_mips()` moved from drain_results (main thread) to worker threads. Workers have spare CPU; the main thread just does fast `queue.write_buffer()` calls.

2. **Pre-sorted eviction queue** — built once per frame, sorted by last_used. Eviction pops from the queue in O(1) instead of scanning 32K+ slots per eviction. Critical when uploading hundreds of bricks per frame.

3. **Spatial demand bounds** — `compute_demand()` only walks cells within ±50 bricks of the camera instead of all 262K cells. Reduces demand computation from ~262K iterations to ~160K.

4. **Resident eviction** — when no MipOnly bricks exist (common after preload), the pager evicts the oldest Resident brick. Previously it would fail silently.

### GPU worldgen coarse mip population

After `GpuWorldGenerator::generate_all()`, a new `populate_coarse_mips()` method iterates all non-air entries in the GPU cache (149K cells), computes full mips, extracts levels 2–4, and bulk-uploads to the coarse mip grid. This runs once during setup (~590 ms) so distant terrain renders from the first frame.

## Memory budget

| Component | Size |
|---|---|
| Voxel pool (131K) | 512 MB |
| Palette pool (131K) | 128 MB |
| Mip pool (131K) | 293 MB |
| Coarse mip grid (262K) | 73 MB |
| BrickIndex (262K) | 1 MB |
| **Total** | **~1 GB** |

## Performance

On M1 Max, Large World preset:
- GPU worldgen: 1.6s (262K cells)
- Coarse mip population: 590ms (150K non-air cells)
- Preload radius 80m: 68K bricks in 2s
- Runtime: 217 FPS avg, 4.7ms CPU, hitch-free streaming at 128 uploads/frame

## What this revealed

The fine-to-coarse generation approach (generate 16³ voxels, compute mips, store coarse for distance) doesn't scale to km worlds. The correct architecture is coarse-to-fine: generate at the coarsest level everywhere, progressively enhance near the camera. This is the next epic.
