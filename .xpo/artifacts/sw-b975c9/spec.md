## What

A GPU-side BVH (bounding volume hierarchy) over object instance AABBs that replaces the linear scan in the raymarcher. Built on the CPU, uploaded as a flat array of nodes alongside the instance table. Refit per frame when transforms change.

## Why

The linear scan tests every instance per ray (primary + shadow). At 300+ instances this becomes the bottleneck. A BVH reduces per-ray cost from O(N) to O(log N) AABB tests.

## Acceptance Criteria

- BVH built from instance AABBs, uploaded as a flat GPU buffer
- Shader traverses BVH instead of linear instance scan (both primary and shadow rays)
- Correct rendering: identical visual output to the linear scan
- Can handle 0 instances gracefully (no BVH, skip traversal)
- `cargo clippy` and `cargo test` pass

## Flow

### 1. BVH builder — `crates/engine/src/bvh.rs`

Flat array BVH with sorted build (surface area heuristic or simple midpoint split):

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BvhNode {
    aabb_min: [f32; 3],
    left_or_first: u32,  // internal: left child index, leaf: first instance index
    aabb_max: [f32; 3],
    count: u32,          // 0 = internal node, >0 = leaf with `count` instances
}
```

- `build(instances: &[(Vec3, Vec3)]) -> Vec<BvhNode>` — builds from AABBs
- Midpoint split along the longest axis, recurse until leaf has ≤ 4 instances
- Flat array layout: node 0 is root, children at `left_or_first` and `left_or_first + 1`

### 2. Scene changes — `scene.rs`

- Add `bvh_buf: Option<wgpu::Buffer>` to `Scene`
- Build BVH during `upload()` from instance world AABBs
- Expose `bvh_buffer()` and `bvh_node_count()`

### 3. Shader changes — `raymarch.wgsl`

Replace the linear instance loop with stack-based BVH traversal:

```wgsl
@group(0) @binding(7) var<storage, read> bvh_nodes: array<BvhNode>;

fn trace_objects_bvh(ro, rd, max_t) -> HitResult {
    var stack: array<u32, 32>;
    var sp = 0u;
    stack[0] = 0u; sp = 1u; // push root
    
    while sp > 0u {
        sp -= 1u; let node_idx = stack[sp];
        let node = bvh_nodes[node_idx];
        
        let hit = ray_aabb(ro, inv_rd, node.aabb_min, node.aabb_max);
        if hit.x >= hit.y || hit.x >= best_t { continue; }
        
        if node.count > 0 { // leaf
            for i in node.left_or_first .. node.left_or_first + node.count {
                // test instance i (same as current linear code)
            }
        } else { // internal
            stack[sp] = node.left_or_first; sp += 1;
            stack[sp] = node.left_or_first + 1; sp += 1;
        }
    }
}
```

Same BVH traversal for shadow rays.

### 4. Raymarcher — `raymarcher.rs`

- Add binding 7 (BVH nodes buffer) to compute layout
- Pass BVH buffer (or dummy) in bind group

### 5. Uniform

Add `bvh_node_count: u32` to uniform (or reuse a pad field).

## Decisions

**D1: CPU-built, GPU-traversed.** Building on the CPU is simpler and fast enough for hundreds of instances. GPU build would only matter at thousands.

**D2: Midpoint split, not SAH.** At <1000 instances, midpoint split is nearly as good as SAH and much simpler. One sort per level, no cost evaluation.

**D3: Flat array layout, not pointer-based.** Children at indices `left` and `left+1`. Cache-friendly, trivial GPU upload, no pointer fixup.

**D4: Stack depth 32.** Supports 2^32 nodes theoretically. At <1000 instances the tree is ~10 levels deep.

## Assumptions

- 39 instances builds in microseconds
- BVH node buffer is small (<10 KB at hundreds of instances)
- Stack of 32 in the shader is sufficient for any practical tree depth
