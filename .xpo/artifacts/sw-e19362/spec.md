
# Large World Preset with Persistent Coarse Mip Grid

## What

A persistent coarse mip grid that retains mip levels 2–4 (73 u32 per cell) for every loaded brick, independent of pool eviction. Distant terrain renders at low resolution instead of vanishing. Plus a Large World preset (128×16×128) that exercises streaming + eviction.

## Why

With a 32K brick pool and 262K cells, evicted bricks currently vanish (grid cell cleared, mip data overwritten). This creates floating surface bricks at distance because underground support bricks are evicted. The fix: every loaded brick permanently stores coarse mip data in a grid-parallel buffer. The shader falls back to this data when the full brick is evicted.

## How

### Engine: `coarse_mip_grid.rs`

A flat buffer parallel to `BrickIndex`, storing mip levels 2–4 per grid cell.

```rust
pub const COARSE_MIP_WORDS: u32 = 73; // 4³ + 2³ + 1³ = 64 + 8 + 1

pub struct CoarseMipGrid {
    buffer: wgpu::Buffer,
    data: Vec<u32>,
    dims: [u32; 3],
}
```

- `write_cell(pos, mips)` — copies levels 2–4 from a full 585-word mip array into the grid
- `upload(queue)` — pushes the CPU mirror to GPU
- `buffer()` — for shader bind group
- Size: 262K × 73 × 4 = **76.6 MB** for the large world (well within 128MB binding limit)

### Engine: shader changes (`raymarch.wgsl`)

New binding: `@group(0) @binding(9) var<storage, read> coarse_mips: array<u32>;`

In `trace_grid`, when `grid_cell == EMPTY`:
```wgsl
let coarse_idx = cell_flat * COARSE_MIP_WORDS;
let top_mip = coarse_mips[coarse_idx + 72]; // level 4: single RGBA
if (top_mip != 0u) {
    // This cell has persistent mip data — shade from it
    let lod = ...; // select level 2, 3, or 4 based on SSE
    let color = sample_coarse_mip(coarse_idx, local_uv, lod);
    // shade and accumulate
}
```

The LOD selection uses the same SSE formula. Levels map to:
- Level 2 (4³): moderate distance, 64 sub-brick colors
- Level 3 (2³): far distance, 8 sub-brick colors  
- Level 4 (1³): very far, single average color

### Engine: `brick_pager.rs` changes

- `update()` and `preload_*()`: when uploading a brick, also write coarse mips to the grid
- On eviction (MipOnly → Unknown): clear the grid cell BUT leave coarse mip data intact
- Takes `&mut CoarseMipGrid` alongside `&mut BrickIndex`

### Engine: `raymarcher.rs` changes

- Add binding 9 for the coarse mip buffer
- Pass `CoarseMipGrid::buffer()` to bind group creation

### Sandbox: Large World preset

- Grid: 128×16×128 (262K cells)
- Pool: 32K (max for 128MB binding limit)
- `preload_radius(50m)` for initial view
- Camera path: expanding spiral (60–90m radius) exercising streaming + eviction

## Memory budget

| Component | Size |
|---|---|
| Voxel pool (32K) | 128 MB |
| Palette pool (32K) | 32 MB |
| Mip pool (32K) | 73 MB |
| **Coarse mip grid (262K)** | **77 MB** |
| BrickIndex (262K) | 1 MB |
| **Total** | **311 MB** |

Well within MGMishMash's ~6GB reference budget.

## Acceptance criteria

- [ ] `CoarseMipGrid` struct with grid-parallel buffer (73 u32/cell)
- [ ] Pager writes coarse mips on brick upload, retains on eviction
- [ ] Shader falls back to coarse mip when grid cell is EMPTY
- [ ] LOD selection for coarse mips (levels 2/3/4 by SSE)
- [ ] Large World preset: continuous terrain at all distances (no floating bricks)
- [ ] No buffer binding limit violations
- [ ] Streaming + eviction visible in bench mode
