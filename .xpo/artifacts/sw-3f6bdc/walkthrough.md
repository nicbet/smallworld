## What was built

The `VoxelVolume` trait and its supporting types in `crates/engine/src/volume.rs` — the abstraction boundary that every OOC pipeline stage programs against. Also migrated the engine's AABB representation from ad-hoc `(Vec3, Vec3)` tuples to a GPU-native `AABB` struct.

## Why

The engine had two implicit volume types (SVO terrain, FlatGrid objects) with hardcoded bind group layouts in the raymarcher. DESIGN.md specifies a trait that exposes enough structure for shader-level traversal optimization without being fully opaque. This trait is the seam — Resolve populates volumes, Cull reads their AABBs, Execute dispatches per `VolumeKind`.

## How the pieces fit together

### New: `crates/engine/src/volume.rs`

**`VolumeKind`** — enum discriminant for shader dispatch:
- `SVO` — sparse voxel octree with occupancy masks
- `FlatGrid` — uniform 3D array with DDA (instanced props)
- `ChunkedDDA` — 32³ brick chunks with BVH macro-skip (future)

The renderer `match`es on this to select a compute kernel. No vtable dispatch on the hot path.

**`AABB`** — GPU-native bounding box, 32 bytes:
```rust
#[repr(C)]
#[derive(bytemuck::Pod, bytemuck::Zeroable)]
pub struct AABB {
    pub min: [f32; 3],
    pub _pad0: f32,
    pub max: [f32; 3],
    pub _pad1: f32,
}
```
Padded to vec4 boundaries for direct GPU upload. `From<(Vec3, Vec3)>` for ergonomic conversion from existing code. `AABB::EMPTY` sentinel for degenerate/empty volumes.

**`LODMeta`** — SSE evaluation metadata:
- `voxel_scale` — finest voxel edge in metres (0.1 terrain, 0.025 props)
- `max_depth` — LOD levels available (SVO tree depth, or 1 for flat grids)

**`TraversalBindings`** — GPU buffers for traversal, kind + buffer slice. The renderer knows the per-kind layout; the trait just hands over the buffers.

**`VoxelVolume`** — object-safe trait:
- `volume_kind()` → `VolumeKind` (constant for lifetime)
- `bounds()` → `AABB` (world-space enclosure)
- `lod_hint()` → `LODMeta` (SSE metadata)
- `traversal_bindings()` → `TraversalBindings<'_>` (GPU buffers)

### Changed: `crates/engine/src/bvh.rs`

`build()` signature changed from `&[(Vec3, Vec3)]` to `&[AABB]`. Internal logic unchanged — converts to `Vec3` at the boundary for centroid/extent computation.

### Changed: `crates/engine/src/world.rs`

`World::extract()` builds `Vec<AABB>` instead of `Vec<(Vec3, Vec3)>` for BVH construction. Uses `AABB::from()` conversion from `VoxelInstance::world_aabb()` output.

## Key decisions

- **Acronyms stay capitalized** — `SVO`, `DDA`, `AABB`, `LOD` are acronyms, not words. `#[allow(clippy::upper_case_acronyms)]` at module level. Readability over style lint for engine-domain types.

- **Trait-only, no implementations** — `Svo` and `VoxelModel` don't implement `VoxelVolume` yet. That's future work when those types are refactored to fit the trait contract.

- **`AABB` is `bytemuck::Pod` from day one** — the AABB buffer (sw-fdfee5) needs GPU-uploadable AABBs. No conversion step later.

- **`TraversalBindings` uses `&[&wgpu::Buffer]`** — simpler than a typed enum with per-variant fields. The renderer already knows the per-kind layout. Extensible without changing the trait.

## Non-obvious details

- `AABB::EMPTY` uses `(INFINITY, NEG_INFINITY)` — standard sentinel for "no extent." `is_empty()` checks `min > max` on any axis. The culling pipeline should skip empty AABBs.

- The trait is object-safe: all methods use `&self` and return owned/borrowed data, no generics or associated types. `dyn VoxelVolume` works for heterogeneous volume collections.
