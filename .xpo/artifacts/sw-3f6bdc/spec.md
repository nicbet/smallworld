## What

Define the `VoxelVolume` trait and `VolumeKind` enum in the engine crate. This is the trait seam that every OOC pipeline stage programs against — Resolve populates volumes, Cull reads their AABBs, Execute dispatches optimized traversal kernels per `VolumeKind`.

## Why

The engine currently has two implicit volume types (SVO terrain, FlatGrid objects) with hardcoded bind group layouts in the raymarcher. DESIGN.md specifies three volume kinds (SVO, FlatGrid, ChunkedDDA) with a trait that exposes enough structure for shader-level optimization without being fully opaque. The trait is the abstraction boundary — renderers dispatch per-kind without knowing internals, new volume types plug in without changing the renderer.

## Acceptance Criteria

- [ ] `VolumeKind` enum with `Svo`, `FlatGrid`, `ChunkedDda` variants
- [ ] `VoxelVolume` trait with `volume_kind()`, `traversal_bindings()`, `bounds()`, `lod_hint()`
- [ ] Supporting types: `Aabb`, `LodMeta`, `TraversalBindings`
- [ ] Trait is object-safe (`dyn VoxelVolume`)
- [ ] Existing `Svo` and `VoxelModel` types do NOT implement the trait yet (future stories)
- [ ] Documentation on trait contract and implementor obligations
- [ ] `make test` and `make lint` pass

## Design

### VolumeKind

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeKind {
    Svo,
    FlatGrid,
    ChunkedDda,
}
```

The discriminant the renderer uses for shader dispatch. Each variant maps to a different compute kernel / traversal algorithm on the GPU. Kept as a plain enum — the renderer `match`es on it, no vtable dispatch for the hot path.

### Aabb

```rust
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Aabb {
    pub min: [f32; 3],
    pub _pad0: f32,
    pub max: [f32; 3],
    pub _pad1: f32,
}
```

GPU-friendly AABB: 32 bytes, 16-byte aligned, directly uploadable. Padding fields exist for GPU struct alignment (vec4 boundaries). The `Aabb` replaces the current `(Vec3, Vec3)` tuples used for bounds throughout the engine.

Constructor from `glam::Vec3` pair for ergonomics:
```rust
impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self;
    pub fn min_vec3(&self) -> Vec3;
    pub fn max_vec3(&self) -> Vec3;
}
```

### LodMeta

```rust
#[derive(Clone, Copy, Debug)]
pub struct LodMeta {
    pub voxel_scale: f32,
    pub max_depth: u8,
}
```

Metadata for SSE-based LOD decisions in the culling stage:
- `voxel_scale` — finest voxel edge in metres (0.1 for terrain, 0.025 for props). SSE formula: `voxel_scale × focal_length / distance`.
- `max_depth` — maximum LOD levels available (SVO tree depth, or 1 for flat grids).

### TraversalBindings

```rust
pub struct TraversalBindings<'a> {
    pub kind: VolumeKind,
    pub buffers: &'a [&'a wgpu::Buffer],
}
```

The GPU resources a renderer needs to bind for traversal. The buffer list is kind-specific:
- `Svo` → `[svo_nodes, voxels, palettes, masks]`
- `FlatGrid` → `[grid_data, instances, bvh_nodes]`
- `ChunkedDda` → `[chunk_headers, voxels, palettes, masks, bvh_nodes]` (future)

The renderer knows the layout per `VolumeKind` — the trait just hands over the buffers. This is deliberately not fully opaque: the shader needs to know the traversal algorithm, and the bind group layout follows from that.

### VoxelVolume trait

```rust
pub trait VoxelVolume {
    fn volume_kind(&self) -> VolumeKind;
    fn bounds(&self) -> Aabb;
    fn lod_hint(&self) -> LodMeta;
    fn traversal_bindings(&self) -> TraversalBindings<'_>;
}
```

Object-safe. Each method returns owned/borrowed data, no generics or associated types.

**Contract:**
- `volume_kind()` — returns the discriminant matching the GPU traversal kernel. Must be constant for the lifetime of the volume.
- `bounds()` — world-space AABB enclosing all solid voxels. Updated when the volume's spatial extent changes. Used by the culling pipeline.
- `lod_hint()` — LOD metadata for SSE evaluation. `voxel_scale` is the finest resolution this volume can render at; `max_depth` is the number of LOD levels available.
- `traversal_bindings()` — GPU buffers needed for traversal, in the order the shader expects. The renderer creates bind groups from these. Buffers must be valid for the current frame.

## Flow

1. **Create `volume.rs`** in engine — `VolumeKind`, `Aabb`, `LodMeta`, `TraversalBindings`, `VoxelVolume` trait.
2. **Add `pub mod volume` to `lib.rs`**.
3. **Migrate `bvh::build` to use `Aabb`** — currently takes `&[(Vec3, Vec3)]`. Update to take `&[Aabb]` and adjust `world.rs` and `voxel_object.rs` accordingly.
4. **Test** — unit tests for `Aabb` construction and conversion. Trait object coercion test.
5. **Lint** — `make lint`.

## Decisions

- **Trait-only, no implementations yet** — the existing `Svo` and `VoxelModel` types don't implement `VoxelVolume` in this story. Implementations come when we refactor those types to fit the trait contract (separate stories). This keeps the change small and reviewable.

- **`TraversalBindings` uses `&[&wgpu::Buffer]` not a typed enum** — the renderer already knows the per-kind buffer layout (it wrote the bind group layout). A buffer slice is simpler than a typed enum with per-variant fields, and it's extensible without changing the trait signature.

- **`Aabb` is GPU-native, not `glam::Vec3` pairs** — the AABB buffer (sw-fdfee5) needs GPU-uploadable AABBs. Making `Aabb` the canonical type from day one means no conversion step when building the GPU buffer. `bytemuck::Pod` + `#[repr(C)]` ensures it's directly castable.

- **`LodMeta` is minimal** — just `voxel_scale` and `max_depth`. The SSE formula (`voxel_scale × focal_length / distance`) is computed by the culling stage, not the volume. More fields can be added as the culling pipeline matures.

- **Object-safe** — the trait uses `&self` and returns owned/borrowed data, no generics. This allows `dyn VoxelVolume` for heterogeneous volume collections (the World may hold different volume types).

## Edge Cases

- **Empty volume** — `bounds()` should return a degenerate AABB (min > max or zero-size). The culling pipeline must handle this gracefully (skip, don't crash).

## Assumptions

- The three `VolumeKind` variants cover all planned volume types. New variants can be added as the engine grows.
- The shader bind group layout per `VolumeKind` is stable within a major version — changing it means updating both the trait implementor and the renderer.
