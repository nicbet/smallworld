
# SVO Raymarcher: Octree Descent Replacing Grid DDA

## What

Replace `trace_grid` (flat grid DDA) with `trace_svo` (stack-based octree descent). The ray descends the tree and stops at the level where the node is sub-pixel. At leaf nodes with bricks, the existing `trace_brick` handles fine DDA. At interior nodes, shade from the node's averaged color. Seamless LOD by construction.

## Why

The flat grid DDA treats every cell at the same resolution. LOD required a separate mip system that created seams at brick boundaries. The SVO traversal IS the LOD system — each interior node carries the correct averaged color for its entire subtree.

## Shader changes

### New uniforms

Replace grid-specific uniforms with SVO uniforms:

```wgsl
struct Uniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    resolution: vec2<f32>,
    _pad0: vec2<f32>,
    world_min: vec3<f32>,
    world_size: f32,           // was brick_size
    grid_dims: vec3<u32>,      // kept for instanced objects
    flags: u32,
    instance_count: u32,
    focal_length: f32,
    sse_threshold: f32,
    svo_root: u32,             // was _pad3
}
```

### New binding

```wgsl
@group(0) @binding(1) var<storage, read> svo_nodes: array<SvoNode>;

struct SvoNode {
    children: u32,
    brick: u32,
    color: u32,
    flags: u32,
}
```

Replaces `grid_map: array<u32>`. Same binding slot.

### `trace_svo` function

Stack-based octree traversal:

```
fn trace_svo(ro, rd, max_t) -> HitResult:
    stack = [(root, world_min, world_size)]
    best = no_hit()
    
    while stack not empty:
        (node_idx, node_min, node_size) = stack.pop()
        node = svo_nodes[node_idx]
        
        // AABB test
        hit = ray_aabb(ro, inv_rd, node_min, node_min + node_size)
        if miss or hit.t >= best.t: continue
        
        // SSE check — is this node sub-pixel?
        dist = max(hit.t, 0.001)
        sse = node_size * focal_length / dist
        
        if sse < sse_threshold OR node has no children:
            // Shade from this node's color
            if node.color alpha > 0:
                unpack color, compute lighting with hit normal
                if hit.t < best.t: best = this hit
            continue
        
        // Has children — descend
        // If this is a leaf with a brick, try fine DDA first
        if node.brick != EMPTY:
            result = trace_brick(ro, rd, hit.t, node.brick, node_min, normal, voxel_scale)
            if result.hit and result.t < best.t: best = result
            continue
        
        // Push valid children (front-to-back order for early termination)
        for each valid child (ordered by distance):
            stack.push(child_idx, child_min, node_size/2)
    
    return best
```

### Front-to-back child ordering

For efficient early termination, push children in back-to-front order (so the closest pops first). The octant ordering is determined by the ray direction signs:

```wgsl
let order = select(0u, 1u, rd.x < 0.0)
          | select(0u, 2u, rd.y < 0.0)
          | select(0u, 4u, rd.z < 0.0);
// XOR each child octant with order to get front-to-back
```

### `trace_brick` returns the true voxel hit t

`trace_brick` previously returned `t_enter` (the brick AABB entry t) as the hit t. That corrupted `best.t` front-to-back culling and brick-vs-instance ordering — terrain could occlude objects actually in front of it. The DDA now tracks the crossing t of the current voxel (`t_local` = the `t_max` component chosen at each step, captured before increment) and returns `t_enter + bias + t_local`. Tracked as bug sw-1293d8.

## Rust changes

### `raymarcher.rs`

- Add `svo_root: u32` to `Uniforms`
- Change binding 1 from `grid_map` buffer to `svo_nodes` buffer
- `new()` and `resize()` take `&Svo` instead of `&BrickIndex`
- `render()` passes `svo.root()` and `svo.world_size()` as uniforms

### `Svo::insert_brick` — exact integer descent (chasm fix)

Brick min corners lie mathematically **on** octree split planes. The original insertion classified octants with `pos >= center` where `center` was accumulated by f32 halving while `pos` came from `world_min + g * BRICK_SIZE` — two rounding paths that disagree by up to ~4e-7 at specific planes. Each disagreement dropped an entire brick plane into the neighboring cell (overwriting it and leaving its own cell empty), producing grid-aligned chasms at x/z indices 7, 14, 15, 19, 20, 23, 25, 28, 30 and y indices 5, 7 in the Default preset (see artifact `chasm-fp-analysis.md`).

`insert_brick` now:

1. Derives `depth = round(log2(world_size / leaf_size))`.
2. Snaps `world_pos` to integer leaf coordinates once, in f64, with `round()` — robust to ±half a cell of accumulated error.
3. Descends by bit tests on the integer coordinates (`(c >> level) & 1`) — no float comparisons in the descent, exact at any tree depth (matters for the 1 km target world).

The shader still reconstructs cell geometry by f32 halving; that is self-consistent and only offsets rendered cell bounds by ~1 µm, which is invisible. Regression test `min_corner_inserts_land_in_distinct_leaves` inserts the full 32×12×32 Default grid with the preset's exact arithmetic and asserts one distinct brick leaf per cell.

### Sandbox integration

Default/TerrainOnly generate bricks via the existing GPU worldgen and insert them into the SVO through `BrickPager::preload_all`. The pager stays active for streaming (`update` → `insert_brick` on load); this deviates from the original plan to disable it. Known gap: eviction leaves a stale brick handle in the SVO leaf — inert in current presets (pool capacity exceeds demand), tracked as sw-fcea39 for the pager adaptation in sw-f089de.

## Acceptance criteria

- [x] `trace_svo` shader function with stack-based octree descent
- [x] SSE-driven LOD: interior nodes shade from averaged color when sub-pixel
- [x] Leaf nodes with bricks use existing `trace_brick` for fine DDA
- [x] Front-to-back child ordering for early ray termination
- [x] Shadow rays traverse the SVO
- [x] No visible seams or LOD transitions at any distance (chasm fix verified manually)
- [x] Default preset renders correctly via SVO
- [ ] Performance: ≥ 30 FPS on Default preset (32×12×32 grid)
