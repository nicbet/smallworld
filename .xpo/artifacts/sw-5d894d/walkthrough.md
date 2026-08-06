## What was built

Data types and scene infrastructure for instanced voxel objects with per-instance voxel scale.

## How the pieces fit together

### VoxelModel (`voxel_object.rs`)

Shared voxel data for an object type. Stores a flat 3D grid of brick pool handles (same global pool as terrain) plus a `voxel_scale`. Multiple instances reference the same model — 200 trees share one model's ~100 bricks.

`fill_brick()` is a convenience method that generates voxels, allocates from the pool, uploads, and sets the grid entry in one call.

`world_extent()` computes the model's size in metres from `dims × brick_edge × voxel_scale`.

### VoxelInstance (`voxel_object.rs`)

CPU-side: model index + position + rotation quaternion. `transform()` builds a mat4 that centers the model at the origin (offset by -extent/2) then rotates and translates. `world_aabb()` transforms all 8 corners of the model extent and takes the min/max.

GPU-side: `VoxelInstanceGpu` is a 160-byte packed struct (two mat4 for transform/inverse, AABB with voxel_scale and grid_offset packed in w components, grid dims). The `grid_offset` indexes into the packed grid buffer so the shader finds each object's local brick grid.

### Scene (`scene.rs`)

Holds `Vec<VoxelModel>` and `Vec<VoxelInstance>`. `upload()` concatenates all model grids into one packed GPU buffer and builds the instance data buffer. Grid offsets are computed during packing — each model's grid data is appended sequentially, and each instance records its model's offset.

### Model generators (`model_gen.rs`)

`generate_tree()` — 2.5cm voxel scale. Trunk is a cylinder ~5 voxels wide (12cm), 120-200 voxels tall (3-5m), with an ellipsoidal leaf canopy using 3 green shades for visual variation.

`generate_rock()` — 2.5cm voxel scale. Ellipsoid with 3 gray shades, 20-40 voxels across (0.5-1m).

Both use the shared `BrickPool` and generate deterministically from a seed.

### Viewer integration

`populate_scene()` creates one tree model and one rock model, then scatters instances on the terrain surface using `WorldGenerator::approx_surface_y()` for placement height. Currently 27 trees + 12 rocks = 39 instances. Pool capacity increased to 32768 to accommodate object bricks.

## Key decisions

- **Global brick pool** for all objects and terrain. No per-object allocation. Simplifies the shader (one voxel/palette buffer pair) and enables future brick sharing across instances.

- **Packed grid buffer** rather than per-object buffers. One GPU buffer binding for all object grids. Each instance's `grid_offset` indexes into it.

- **Transform centers the model** — `translate(position) × rotate(rotation) × translate(-extent/2)`. This makes position the center of the object in world space, which is more intuitive for placement and physics.

## What a future reader should know

- Objects are created and uploaded but NOT rendered yet. The traversal story (sw-06bbbc) adds them to the raymarcher.

- `VoxelInstanceGpu::aabb_max.w` stores the grid offset as `f32::from_bits(u32)` — a bit-cast, not a conversion. The shader must use `bitcast<u32>()` to read it.

- The tree model at 2.5cm uses about 60-80 bricks (240-320 KB). Shared across all tree instances, so 100 trees = one model + 100 × 160 bytes of instance data.
