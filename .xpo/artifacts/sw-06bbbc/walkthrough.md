## What was built

Two-level ray traversal that renders instanced voxel objects alongside terrain. Rays find the closest hit across the terrain grid and all object instances, with per-instance voxel scale and transforms.

## How the pieces fit together

### Shader architecture

The main loop now has three phases:

1. **Terrain** — `trace_terrain(ro, rd, max_t)` runs the existing coarse+fine DDA against the terrain grid. Returns the closest hit with a t-value.

2. **Objects** — linear scan over all instances. For each: ray-AABB test against the world-space bounding box, early-exit if farther than best hit, then `trace_object()` transforms the ray into object space and runs the same grid+fine DDA with the object's voxel scale and local grid data.

3. **Shading** — the closest hit (terrain or object) gets normal selection, shadow ray, and final color.

### Object-space ray transformation

The ray is transformed with `inv_transform`: origin as a point (w=1), direction as a vector (w=0). The direction is normalized in object space, and `rd_scale = length(inv_transform × rd)` converts object-space t-values back to world-space for comparison with terrain hits.

Hit results are transformed back: position via `transform × pos`, normal via `normalize(transform × normal)`.

### Object grid access

Each instance stores a `grid_offset` (bitcast as f32 in `aabb_max.w`) indexing into the packed `object_grids` buffer. The object's local grid lookup is `object_grids[grid_offset + flat_index]` instead of `grid_map[flat_index]`.

### Shadow rays

`trace_shadow` tests both terrain and objects. The terrain path is unchanged; the object path does a simplified linear scan with `any_hit_brick` (no shading, early exit on first hit).

### Surface placement

`WorldGenerator::find_surface_y()` scans the density function vertically in 10cm steps to find the actual air→solid transition, replacing the approximate height that caused floating objects.

## Key decisions

- **Linear instance scan** — at 39 instances, the per-pixel cost of 39 AABB tests is negligible. BVH (sw-b975c9) drops in as a replacement for the loop.

- **Dummy buffers** for empty scenes — wgpu requires all bind group entries to be valid, so a 16-byte dummy buffer is bound when the scene has no instances.

- **Removed terrain-baked trees** — the worldgen no longer generates trees in the terrain density function. Trees come exclusively from 2.5cm instanced objects.

## What a future reader should know

- The WGSL `ObjectInstance` struct must match `VoxelInstanceGpu` in `voxel_object.rs` exactly — two mat4x4, two vec4, one vec4<u32>. The `aabb_max.w` stores `grid_offset` as a bitcast u32→f32.

- The uniform struct is 144 bytes (not 160). The `_pad1` fields are individual `u32`s, not a `vec3<u32>`, to avoid WGSL alignment surprises.

- Object traversal reuses the same `trace_brick` function as terrain — the only difference is the `vs` (voxel_scale) parameter.
