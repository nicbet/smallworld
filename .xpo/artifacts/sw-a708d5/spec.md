
# SVO Node Structure + GPU Buffer Layout

## What

Define the Sparse Voxel Octree node format and its flat-array GPU representation. This replaces `BrickIndex` as the spatial index for the voxel world.

## Why

The flat brick grid (BrickIndex) has no hierarchy — every cell is at the same resolution. LOD requires a separate mip system bolted on top, which creates seams at brick boundaries. An SVO provides LOD by construction: each interior node IS the averaged representation of its children. No seams, no transitions.

## Node format

```rust
#[repr(C)]
struct SvoNode {
    children: u32,    // index into node array where 8 children start (0 = no children / leaf)
    brick: u32,       // BrickHandle gpu_index (u32::MAX = no brick)
    color: u32,       // packed RGBA — averaged color of all descendant voxels
    flags: u32,       // bit 0-7: valid child mask, bit 8: is_leaf
}
```

**16 bytes per node.** Children are stored as a contiguous block of 8 at `nodes[children_idx..children_idx+8]`. Only children with their bit set in the valid mask are non-empty.

### Color computation

Each node's `color` is the occupancy-weighted average of its children's colors. At leaf nodes, the color comes from the brick's voxel data (average of all non-air voxels). At interior nodes, it's recursively averaged from children. This means any node in the tree can be rendered as a single colored voxel at the appropriate size — seamless LOD.

### Tree depth

For 1 km at 10 cm voxels: world edge = 10,000 voxels. Our bricks are 16³, so the tree bottoms out at brick resolution (1.6m per leaf). Below that, the brick's fine DDA handles individual voxels.

Tree levels (brick-resolution leaves):
- Level 0 (root): 1 node covering the entire world
- Level 1: up to 8 nodes (each covers 1/8 of the world)
- ...
- Level ~9: leaf nodes at ~1.6m resolution (625 nodes per axis for 1km)

log2(625) ≈ 9.3, so ~10 levels of octree above the brick level. Total nodes for a 1km world: sparse, maybe 5-20M depending on terrain complexity. At 16 bytes/node: 80-320 MB.

## GPU buffer

```rust
pub struct Svo {
    nodes: Vec<SvoNode>,        // CPU mirror
    buffer: wgpu::Buffer,       // GPU storage buffer
    root: u32,                  // index of root node
    world_min: Vec3,            // world-space origin
    world_size: f32,            // world edge length (power of 2, >= actual world)
}
```

The buffer is a single `storage<read>` binding. The shader traverses by reading `nodes[idx]`, checking the child mask, and descending to `nodes[children_idx + child_octant]`.

### Allocation

Nodes are allocated from a free-list pool (similar to BrickPool but for tree nodes). Children are always allocated as contiguous blocks of 8. This ensures `children_idx + octant` addressing works.

```rust
pub fn alloc_children(&mut self) -> u32 {
    // returns index of first child in a block of 8
}

pub fn free_children(&mut self, idx: u32) {
    // returns block of 8 to free list
}
```

## Shader interface

```wgsl
struct SvoNode {
    children: u32,
    brick: u32,
    color: u32,
    flags: u32,
}

@group(0) @binding(1) var<storage, read> svo_nodes: array<SvoNode>;
```

Replaces the current `@binding(1) grid_map: array<u32>`. The raymarcher reads `svo_nodes` instead of `grid_map`.

## Rust API

```rust
impl Svo {
    pub fn new(device: &wgpu::Device, capacity: u32, world_min: Vec3, world_size: f32) -> Self;
    pub fn insert_brick(&mut self, world_pos: Vec3, brick_handle: BrickHandle, color: [u8; 4]);
    pub fn set_node_color(&mut self, node_idx: u32, color: [u8; 4]);
    pub fn upload(&self, queue: &wgpu::Queue);
    pub fn buffer(&self) -> &wgpu::Buffer;
    pub fn root(&self) -> u32;
    pub fn world_size(&self) -> f32;
}
```

## What this does NOT include

- Raymarching (story 2)
- Construction from noise (story 3)
- Streaming/paging (story 3)
- SVDAG compression (future optimization)

This story delivers the data structure, GPU buffer, and API. A simple unit test builds a small tree by hand and verifies traversal correctness on CPU.

## Acceptance criteria

- [ ] `SvoNode` struct (16 bytes, Pod/Zeroable)
- [ ] `Svo` struct with node pool, free-list allocation, GPU buffer
- [ ] `insert_brick()` creates/subdivides nodes along the path from root to leaf
- [ ] Interior node colors computed as average of children
- [ ] `upload()` pushes to GPU
- [ ] Unit test: build a 4-level tree, verify structure and colors
- [ ] Registered in engine lib.rs as `pub mod svo`
