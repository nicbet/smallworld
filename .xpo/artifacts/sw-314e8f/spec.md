## What

Introduce `World` as the engine's central data structure — the thing every game constructs. It owns entity instances (SlotMap), shared models (Vec), and an extraction boundary that packs world state into GPU-ready buffers. Dirty tracking is built in from day one so the extraction step knows what changed. The existing `Scene` is retired.

This follows the pattern used by all major engines: game code mutates the World, the renderer consumes an extracted snapshot, and the two never share mutable state.

## Why

The current `Scene` is a flat bag of instances with full-rebuild GPU upload. It has no stable handles, no dirty tracking, no extraction boundary, and its name overpromises — it doesn't hold terrain, camera, or spatial hierarchy.

The OOC pipeline needs a central structure that:
- Gives every instance a stable handle (for runtime mutation, streaming lifecycle)
- Knows what changed since last frame (for incremental GPU upload)
- Cleanly separates world state from GPU representation (extraction boundary)
- Is extensible for spatial hierarchy (sw-45a34a), VoxelVolume (sw-3f6bdc), and AABB buffer (sw-fdfee5)

## Acceptance Criteria

- [ ] `World` struct in engine with SlotMap-based instance storage
- [ ] `InstanceKey` returned from `add_instance`, usable for mutation and removal
- [ ] Per-instance generation counter, incremented on mutation
- [ ] `World::extract()` produces `&WorldGpuData` (packed GPU buffers + BVH), no args
- [ ] `extract()` skips work when nothing changed (dirty flag)
- [ ] `Scene` module removed, all consumers migrated to `World`
- [ ] Raymarcher consumes `&WorldGpuData` instead of `&Scene`
- [ ] All sandbox scenes render identically
- [ ] `make test` and `make lint` pass
- [ ] No performance regression (`make bench`)

## Design

### World (game-facing)

```rust
pub struct World {
    device: wgpu::Device,
    queue: wgpu::Queue,
    models: Vec<VoxelModel>,
    instances: SlotMap<InstanceKey, Instance>,
    gpu_data: WorldGpuData,
    dirty: bool,
}
```

`World` is the struct games build. It clones the `wgpu::Device` and `Queue` at construction (both are `Arc`-wrapped internally — cloning is a refcount bump). This means `extract()` needs no arguments:

```rust
let world = World::new(&gpu);
world.add_model(tree);
world.add_instance(Instance { .. });
let gpu_data = world.extract();
renderer.render(gpu_data);
```

Camera, `BrickPool`, `SVO`, `Raymarcher` stay as separate structs owned by the application — World is entity/spatial state, not the entire engine.

### Instance (per-entity data)

```rust
pub struct Instance {
    pub model_id: usize,
    pub position: Vec3,
    pub rotation: Quat,
    generation: u32,
}
```

All runtime-mutable state lives as fields on `Instance` — never as presence/absence of the Instance itself (per the churn rule from sw-cf6350). The `generation` counter increments on any mutation, enabling future per-instance dirty detection.

### WorldGpuData (renderer-facing)

```rust
pub struct WorldGpuData {
    instance_buf: Option<wgpu::Buffer>,
    grid_buf: Option<wgpu::Buffer>,
    bvh_buf: Option<wgpu::Buffer>,
    instance_count: u32,
    generation: u32,
}
```

The output of `extract()`. This is all the renderer needs — packed GPU buffers and a count. The renderer never reaches back into `World`. The `generation` counter lets downstream consumers (bind group creation, etc.) detect when data actually changed.

### Extraction boundary

```rust
impl World {
    pub fn extract(&mut self) -> &WorldGpuData { ... }
}
```

`extract()` is the Godot `RenderingServer` / Unreal `SendAllEndOfFrameUpdates` equivalent. It:
1. Checks `self.dirty` — if false, returns cached `&self.gpu_data`
2. Collects instances into a temp Vec (for BVH building)
3. Builds BVH from AABBs
4. Packs GPU instance data in BVH leaf order
5. Creates/uploads GPU buffers using the stored `self.device`
6. Clears dirty flag, bumps `gpu_data.generation`
7. Returns `&self.gpu_data`

Initial implementation does full rebuilds when dirty. The dirty flag + generation counters are the hooks for incremental updates in future work.

### Mutation methods

```rust
impl World {
    pub fn add_model(&mut self, model: VoxelModel) -> usize;
    pub fn add_instance(&mut self, inst: Instance) -> InstanceKey;
    pub fn remove_instance(&mut self, key: InstanceKey) -> Option<Instance>;
    pub fn get(&self, key: InstanceKey) -> Option<&Instance>;
    pub fn get_mut(&mut self, key: InstanceKey) -> Option<&mut Instance>;
    pub fn instances(&self) -> impl Iterator<Item = (InstanceKey, &Instance)>;
    pub fn models(&self) -> &[VoxelModel];
    pub fn instance_count(&self) -> u32;
}
```

Every mutation method sets `self.dirty = true`. `get_mut` returns a mutable reference — the caller can modify position/rotation directly. We bump the instance's `generation` inside a `modify()` wrapper or trust the caller (simpler for now; we can add a `Mut<'_, Instance>` guard later if needed).

## Flow

1. **Add `slotmap` to workspace** — `slotmap = "1"` in workspace `Cargo.toml` and engine deps.

2. **Create `world.rs`** — `World`, `Instance`, `InstanceKey`, `WorldGpuData`, extraction logic. Move BVH building and GPU packing from `scene.rs` into `extract()`.

3. **Migrate raymarcher** — replace `scene: &Scene` parameters with `gpu_data: &WorldGpuData`. The raymarcher reads `gpu_data.instance_buffer()`, `gpu_data.grid_buffer()`, `gpu_data.bvh_buffer()`, `gpu_data.instance_count()`.

4. **Migrate sandbox** — `main.rs`: replace `scene: Scene` with `world: World` in `RunState`. `scenes.rs`: populate `World` instead of `Scene`. Call `world.extract()` before passing to raymarcher.

5. **Remove `scene.rs`** — delete the module and its `pub mod` in `lib.rs`.

6. **Update VoxelInstance** — rename to `Instance` or keep as-is and re-export. The GPU-side `VoxelInstanceGpu` stays unchanged (it's a GPU format, not a world concept).

7. **Test** — `make test`, `make lint`, visual check with `make sandbox`, `make bench`.

## Decisions

- **World stores Device + Queue clones** — `wgpu::Device` and `wgpu::Queue` are `Arc`-wrapped internally; cloning is a refcount bump. World gets them at construction, `extract()` uses them without any arguments. This eliminates the need to thread `&device` through every call site. Camera, BrickPool, SVO, Raymarcher stay as separate application-owned structs — World is not the entire engine, just the entity/spatial layer.

- **Full rebuild on dirty, not incremental** — the dirty flag + generation counters establish the mechanism for incremental updates, but this story implements full rebuild. Incremental BVH update and partial buffer writes are future optimization work when profiling shows `extract()` is a bottleneck.

- **`generation` on Instance, not a dirty bitset** — a per-instance generation counter is more useful than a dirty bit. It lets future code ask "has this instance changed since I last looked?" without a central coordinator. Cost: 4 bytes per instance (negligible at our scale).

- **`VoxelInstanceGpu` stays in `voxel_object.rs`** — it's a GPU format concern, not a world concept. `extract()` calls `VoxelInstanceGpu::from_instance()` during packing. The type doesn't move.

- **Models stay as `Vec<VoxelModel>` with usize keys** — models are shared data, added at load time, never removed. SlotMap overhead isn't justified.

## Edge Cases

- **Empty world** — `extract()` on a world with zero instances should produce `WorldGpuData` with `None` buffers and `instance_count = 0`. The raymarcher already handles this (falls back to dummy buffers).

- **Extract before any mutation** — calling `extract()` on a freshly constructed World should work. `dirty` starts as `false`, cached data is empty, renderer gets zero instances.

- **Remove during iteration** — not needed yet, but `SlotMap::remove` is O(1) and doesn't invalidate other keys. Future streaming code can safely remove instances while iterating a collected key list.

## Assumptions

- `slotmap` 1.x is stable and a reasonable long-term dependency.
- `wgpu::Device` and `wgpu::Queue` are cheap to clone (Arc internals) — verified in wgpu 30.
- The raymarcher's bind group layout doesn't change — it still expects the same three storage buffers (instances, grids, BVH nodes). Only the source of those buffers changes.
- No existing code references instances by Vec index after creation — confirmed by grep.

## Future extension points

- **Spatial hierarchy** (sw-45a34a) — World gets a spatial index field (octree/BVH) built from instance AABBs, used for frustum culling and streaming decisions.
- **VoxelVolume trait** (sw-3f6bdc) — models may become trait objects behind VoxelVolume; World stores them generically.
- **AABB buffer** (sw-fdfee5) — `extract()` produces an AABB buffer alongside instance/BVH buffers for GPU culling passes.
- **Incremental extract** — per-instance generation comparison against last-extracted generation; partial buffer updates via `write_buffer` offsets.
