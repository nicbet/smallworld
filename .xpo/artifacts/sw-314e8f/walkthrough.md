## What was built

Replaced the `Scene` struct with `World` — the engine's central data structure for instanced voxel objects. `World` owns instances via `SlotMap` with stable handles, stores a cloned `wgpu::Device` internally, and exposes an extraction boundary (`extract()`) that produces GPU-ready data for the renderer.

## Why

`Scene` was a flat `Vec<VoxelInstance>` with no stable handles, no dirty tracking, and a full-rebuild `upload()` that took `&device` as an argument. The OOC pipeline needs stable entity references (for runtime mutation, streaming lifecycle), a clear separation between world state and GPU representation (extraction boundary), and extensibility for spatial hierarchy and culling buffers in later issues.

The design follows the pattern used by Unity (World + BRG extraction), Unreal (UWorld + FScene proxy), and Godot (SceneTree + RenderingServer): game code mutates the World, the renderer consumes an extracted snapshot, the two never share mutable state.

## How the pieces fit together

### New: `crates/engine/src/world.rs`

```
World
├── device: wgpu::Device        (Arc clone, cheap)
├── models: Vec<VoxelModel>     (shared data, usize keys)
├── instances: SlotMap<InstanceKey, VoxelInstance>
├── gpu_data: WorldGpuData      (cached extraction output)
└── dirty: bool                 (skip extract when clean)
```

**`InstanceKey`** — stable handle from `slotmap::new_key_type!`. Returned by `add_instance()`, usable for `get()`, `get_mut()`, `remove_instance()`. Survives other insertions/removals.

**`WorldGpuData`** — the renderer-facing snapshot:
- `instance_buffer()`, `grid_buffer()`, `bvh_buffer()` — same three storage buffers the raymarcher expects
- `instance_count()` — for uniform upload
- `generation()` — monotonic counter, bumped on each rebuild. Future use: bind group cache invalidation.

**`extract()`** — the extraction boundary. Checks `dirty`; if false, returns `&self.gpu_data` immediately. Otherwise: collects instances, packs model grids, builds BVH, uploads buffers, clears dirty, bumps generation. Uses the stored `self.device` — no arguments needed.

### Deleted: `crates/engine/src/scene.rs`

All logic moved into `World::extract()`. The BVH building and GPU packing code is structurally identical — the only change is iterating a SlotMap instead of a Vec.

### Changed: `crates/engine/src/raymarcher.rs`

Every `scene: &Scene` parameter became `world_data: &WorldGpuData`. The raymarcher reads the same three buffer accessors — the interface is identical, just the source type changed. Fixed a pre-existing duplicated `#[allow(clippy::too_many_arguments)]` attribute.

### Changed: `crates/sandbox/src/main.rs`

`RunState.scene: Scene` → `RunState.world: World`. Construction: `World::new(&gpu.device)`. Extraction: `self.world.extract()` returns `&WorldGpuData` passed to raymarcher. The `load_preset` flow became:
```rust
self.world = World::new(&self.gpu.device);
preset.setup(..., &mut self.world);
let world_data = self.world.extract();
Raymarcher::new(..., world_data);
```

### Changed: `crates/sandbox/src/scenes.rs`

All `scene: &mut Scene` parameters → `world: &mut World`. `scene.add_model/add_instance` → `world.add_model/add_instance`. The returned `InstanceKey` is unused by current presets (fire-and-forget spawning).

### Changed: `crates/sandbox/src/coarse_svo.rs`

Two ignored test functions (`render_default_headless`, `bench_raymarch`) migrated from `Scene` to `World`.

### Removed: `crates/bench-ecs/`

The ECS A/B spike crate was removed after the decision was made and documented. Benchmark data is preserved in the xpo artifact on sw-cf6350.

## Key decisions

- **Device stored on World** — `wgpu::Device` is `Arc`-wrapped internally; cloning is a refcount bump. This eliminates threading `&device` through every call site. `extract()` takes no arguments.

- **Dirty flag, not per-instance tracking** — a single `dirty: bool` triggers full rebuild. Per-instance generation counters are a future optimization when profiling shows `extract()` is a bottleneck. The generation field on `WorldGpuData` lets downstream consumers detect changes without comparing buffers.

- **SlotMap, not DenseSlotMap** — default `SlotMap` provides the right trade-off. `DenseSlotMap` packs tighter for iteration but is slower on insert/remove. We don't iterate instances every frame (only on extract when dirty).

- **Models stay as Vec** — shared data, added at load time, never removed. `usize` indexing is simpler than SlotMap for this access pattern.

## Non-obvious details

- `extract()` collects `SlotMap` entries into a temp `Vec` for BVH building. The BVH reorders by spatial locality; the GPU gets a flat packed array in BVH leaf order. SlotMap keys are not preserved in GPU data — the GPU doesn't need them.

- All mutation methods (`add_instance`, `remove_instance`, `get_mut`, `add_model`) set `dirty = true`. A freshly constructed World has `dirty = false` and empty `WorldGpuData`, which the raymarcher handles via dummy buffer fallbacks.
