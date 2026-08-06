## What

Data types for instanced voxel objects: a `VoxelModel` (shared voxel data in a local brick grid at a given voxel scale) and a `VoxelInstance` (model reference + rigid transform + world-space AABB). A `Scene` struct holds the terrain plus a list of instances. Includes procedural test models (tree, rock) at 2.5cm voxel scale to prove per-instance scale works.

## Why

Everything after this — BVH traversal, destruction, debris, props — builds on the object/instance separation. Defining the types now means the next two stories (BVH + traversal) have clean data to work with.

## Acceptance Criteria

- `VoxelModel` stores a local brick grid, dims, and voxel scale; bricks live in the shared `BrickPool`
- `VoxelInstance` stores a model index, transform, inverse transform, and world AABB
- `Scene` holds the terrain `BrickIndex` plus a `Vec<VoxelInstance>` and a `Vec<VoxelModel>`
- Procedural tree model at 2.5cm scale (trunk ~5 voxels wide, round canopy)
- Procedural rock model at 2.5cm scale
- Test scene: terrain + ~100 tree instances + ~50 rock instances scattered on the surface
- Instance data uploadable to a GPU buffer (packed struct, bytemuck-compatible)
- No rendering changes — objects won't be visible until sw-06bbbc (traversal)
- `cargo clippy` and `cargo test` pass

## Flow

### 1. New module — `crates/engine/src/voxel_object.rs`

**`VoxelModel`:**
```rust
pub struct VoxelModel {
    grid: Vec<u32>,       // Flat 3D grid of brick pool handles (u32::MAX = empty)
    dims: [u32; 3],       // Grid dims in bricks
    voxel_scale: f32,     // Metres per voxel (e.g. 0.025 for 2.5cm)
}
```

Methods: `new(dims, voxel_scale)`, `set(pos, handle)`, `get(pos)`, `world_extent() -> Vec3` (dims × brick_edge × voxel_scale), `grid_data() -> &[u32]`.

**`VoxelInstance`:**
```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VoxelInstanceGpu {
    transform: [f32; 16],      // Object → world
    inv_transform: [f32; 16],  // World → object
    aabb_min: [f32; 4],        // World AABB min (w = voxel_scale)
    aabb_max: [f32; 4],        // World AABB max (w = grid_offset into packed buffer)
    grid_dims: [u32; 4],       // xyz = dims, w = brick_count
}
```

CPU-side `VoxelInstance`:
```rust
pub struct VoxelInstance {
    pub model_id: usize,
    pub position: Vec3,
    pub rotation: Quat,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
}
```

With `compute_aabb(&self, model: &VoxelModel)` to derive world AABB from transform + model extent.

### 2. New module — `crates/engine/src/scene.rs`

```rust
pub struct Scene {
    pub models: Vec<VoxelModel>,
    pub instances: Vec<VoxelInstance>,
    packed_grids: wgpu::Buffer,    // All model grids concatenated
    instance_buf: wgpu::Buffer,    // GPU instance data
}
```

Methods:
- `new(device)` — empty scene
- `add_model(model) -> usize` — returns model index
- `add_instance(instance)` — adds an instance
- `upload(device, queue, pool)` — packs model grids + instance data to GPU buffers
- `instance_buffer()`, `grid_buffer()` — for bind groups

### 3. Model generation — `crates/engine/src/model_gen.rs`

- `generate_tree(pool, queue, seed) -> VoxelModel` — tree at 2.5cm: trunk ~5 voxels wide, 120-200 voxels tall (3-5m), ellipsoidal canopy
- `generate_rock(pool, queue, seed) -> VoxelModel` — rounded rock at 2.5cm: 20-40 voxels across (0.5-1m)
- Uses the same `BrickPool` as terrain — bricks are global

### 4. Viewer integration

- Create `Scene`, add tree + rock models, scatter instances on terrain surface
- Store `Scene` in `RunState`
- Debug panel shows instance count
- No rendering — objects are invisible until the traversal story

## Decisions

**D1: Models store handles into the global BrickPool, not their own buffers.** One pool for everything — terrain and objects share brick memory. Simplifies allocation, eviction, and the shader (one voxel/palette buffer pair).

**D2: Packed grid buffer for all models, not per-model buffers.** One GPU buffer binding for all object grids. Each instance's `grid_offset` indexes into the packed buffer. Avoids per-object bind groups.

**D3: Instance GPU struct is 160 bytes (two mat4 + 3 vec4).** Padded for GPU alignment. At 1000 instances = 160 KB — negligible.

**D4: Terrain remains a separate code path (BrickIndex), not an instance.** The terrain is axis-aligned with identity transform — making it an instance would add unnecessary transform overhead to every terrain ray. The TLAS traversal tests objects; terrain is tested separately.

## Assumptions

- 100-200 instances at 2.5cm scale need ~100 bricks per model × ~5 unique models = 500 bricks. Well within pool capacity.
- The packed grid buffer fits in one storage buffer binding.
