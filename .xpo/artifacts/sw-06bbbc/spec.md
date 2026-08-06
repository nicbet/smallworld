## What

Extend the raymarcher to test rays against instanced voxel objects in addition to terrain. Each ray finds the closest hit across terrain and all object instances. Object rays are transformed into object-local space and use the same two-level DDA (coarse grid → fine brick) with per-instance voxel scale. Linear instance scan for now (BVH is sw-b975c9).

## Why

This makes the 2.5cm tree and rock instances visible — the same raymarcher code path renders terrain at 10cm and objects at 2.5cm. Once this works, destruction debris and movable props follow naturally (same traversal, different transform).

## Acceptance Criteria

- Trees and rocks at 2.5cm voxel scale render on the terrain
- Per-instance voxel scale works (objects are visibly higher-resolution than terrain)
- Closest-hit across terrain + objects (objects don't render behind terrain)
- Object shadows work (shadow ray tests objects too)
- Interactive frame rates maintained with ~40 instances
- `cargo clippy` and `cargo test` pass

## Flow

### 1. Shader changes — `raymarch.wgsl`

**New bindings (group 0):**
| Binding | Type | Contents |
|---------|------|----------|
| 5 | storage, read | Instance data (`array<VoxelInstanceGpu>`) |
| 6 | storage, read | Packed object grids (`array<u32>`) |

**New uniform fields:** `instance_count: u32` (repurpose or extend flags field).

**New structs in WGSL:**
```wgsl
struct ObjectInstance {
    transform: mat4x4<f32>,
    inv_transform: mat4x4<f32>,
    aabb_min: vec4<f32>,    // w = voxel_scale
    aabb_max: vec4<f32>,    // w = grid_offset (bitcast u32)
    grid_dims: vec4<u32>,   // w = total cells
}
```

**Modified main loop:**

1. Trace terrain (existing code) → get terrain hit t + color + normal
2. For each instance (linear scan):
   a. Ray-AABB test against instance world AABB → skip if miss or farther than best hit
   b. Transform ray to object space: `obj_ro = inv_transform × world_ro`, `obj_rd = inv_transform × world_rd` (direction only, no normalize to preserve t-values)
   c. Coarse DDA through object's local grid (using instance's grid_dims, voxel_scale, grid_offset into packed buffer)
   d. Fine DDA inside bricks (same code, but reading from shared voxel/palette buffers with per-instance voxel_scale)
   e. If hit and t < best_t: update best hit
3. Shade the closest hit (terrain or object)

**Shadow ray:** Also tests objects — `trace_shadow` gains instance loop.

**Key detail:** The object DDA uses `instance.aabb_min.w` as voxel_scale and reads the grid from `object_grids[grid_offset + flat_index]` instead of `grid_map[flat_index]`.

### 2. Raymarcher changes — `raymarcher.rs`

- Compute bind group layout gains bindings 5 and 6 (instance + object grid buffers)
- `new()` and `resize()` take `&Scene` in addition to pool + index
- `render()` takes `&Scene` and writes `instance_count` to uniform
- Uniform struct gains `instance_count: u32`
- Handle empty scene (no instances) gracefully — bind dummy 1-byte buffers

### 3. Viewer changes — `main.rs`

- Pass `&scene` to raymarcher construction, resize, and render
- No other changes needed — scene is already populated

### 4. Refactor: shared brick DDA function

The fine DDA inside a brick is identical for terrain and objects — only the voxel_scale and buffer source differ. Refactor `trace_brick` to accept voxel_scale as a parameter. The coarse grid DDA is parameterized by grid_dims, grid source (terrain grid_map vs object packed buffer + offset), and brick_size (grid_dims × voxel_scale).

## Decisions

**D1: Linear instance scan, not BVH.** At 39 instances, brute-force AABB testing is negligible vs. the DDA cost. BVH is sw-b975c9 and drops in as a replacement for the loop.

**D2: Transform ray, not voxels.** The ray is transformed into object space (inv_transform × ray) rather than transforming every voxel to world space. One transform per instance vs. thousands per voxel.

**D3: Don't normalize object-space ray direction.** The inv_transform may include scale (from voxel_scale). Normalizing would break t-value comparison between terrain and objects. Instead, convert object-space t back to world-space t by dividing by `length(inv_transform × rd)`.

**D4: Dummy buffers for empty scene.** wgpu requires all bind group entries to be valid. When there are no instances, bind 1-element dummy buffers for the instance and grid slots.

## Assumptions

- 39 instances × AABB test per ray is negligible (~0.1 ms at 1M rays)
- The packed grid buffer and instance buffer from `Scene::upload()` are ready before rendering
- Object voxels use the same global palette buffer (per-brick palettes in the shared pool)
