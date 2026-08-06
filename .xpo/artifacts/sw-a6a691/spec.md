
# Cross-Brick Mip Blending

## What

Trilinear interpolation of mip colors at brick boundaries. When the ray hits a distant brick (SSE < threshold), the shader blends the mip color with neighboring bricks' mip colors near the edges, eliminating the visible grid seams.

## Why

Each brick's mip sub-blocks are averaged independently. At brick boundaries, adjacent bricks can have different average colors, creating hard seams — a visible grid pattern at distance. This is the most prominent visual artifact in the engine.

## How

### New shader function: `sample_blended_mip`

Replaces direct `sample_brick_mip()` calls in the mip path of `trace_grid`.

For each axis (X, Y, Z), check if `local_uv` is within half a mip sub-block of the brick edge. If so, look up the neighbor brick's handle from `grid_map` and blend:

```wgsl
fn sample_blended_mip(
    grid_pos: vec3<i32>, handle: u32, local_uv: vec3<f32>, lod: u32
) -> vec4<f32> {
    var color = sample_brick_mip(handle, local_uv, lod);
    if color.a == 0.0 { return color; }
    
    let half_cell = 0.5 / f32(mip_edge_for_lod(lod));
    
    // For each axis: if near edge, blend with neighbor
    // X axis
    if local_uv.x < half_cell {
        let nh = grid_neighbor(grid_pos, vec3(-1, 0, 0));
        if nh != EMPTY {
            let nc = sample_brick_mip(nh, vec3(local_uv.x + 1.0, local_uv.yz), lod);
            if nc.a > 0.0 {
                color = mix(nc, color, local_uv.x / half_cell);
            }
        }
    } else if local_uv.x > 1.0 - half_cell {
        // +X neighbor, mirror blend
    }
    // Y and Z axes: same pattern
    
    return color;
}
```

### Neighbor lookup

```wgsl
fn grid_neighbor(pos: vec3<i32>, offset: vec3<i32>) -> u32 {
    let np = pos + offset;
    if any(np < vec3(0)) || any(np >= vec3<i32>(u.grid_dims)) {
        return EMPTY;
    }
    return grid_map[u32(np.x) + u.grid_dims.x * (u32(np.y) + u.grid_dims.y * u32(np.z))];
}
```

### Apply to both mip paths

1. **Pool mip path** (Resident bricks, SSE < threshold): use `sample_blended_mip` with `grid_map` handles + `sample_brick_mip`
2. **Coarse mip path** (evicted bricks): use `sample_blended_coarse_mip` with `coarse_mips` buffer

### Performance

Additional cost per mip-shaded pixel: up to 6 neighbor handle lookups (grid_map reads) + up to 6 additional mip buffer reads. Only for pixels near brick edges (within half a sub-block). Interior pixels are unchanged. At mip distances, pixels are cheap (no fine DDA), so doubling the buffer reads is acceptable.

## Acceptance criteria

- [ ] `sample_blended_mip` with per-axis neighbor interpolation
- [ ] `sample_blended_coarse_mip` for coarse path
- [ ] `grid_neighbor` helper
- [ ] No visible grid seams at mip distance on Default, TerrainOnly, Large World
- [ ] No performance regression > 10% on benchmark
