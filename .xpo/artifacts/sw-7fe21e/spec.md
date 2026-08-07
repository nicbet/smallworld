## What

First stage of the OOC pipeline. Takes World + view parameters, produces a `VisibilitySet` — the subset of volumes, mesh instances, and lights that need rendering this frame. Initial implementation is a no-op passthrough (everything visible). The interface is ready for frustum culling, HZB occlusion, and SSE-based LOD evaluation.

## Why

Pipeline stages downstream (Stream, Execute) must operate on the visible set, not the full world. Without culling, a scene with thousands of volumes and mesh instances would process all of them every frame. Even the no-op implementation establishes the data flow: World → Cull → VisibilitySet → Stream → Execute.

## Acceptance Criteria

- `VisibilitySet` type with visible volume keys, visible mesh instance keys, visible light keys
- `CullStage` struct with `cull(&self, world, view) -> VisibilitySet`
- No-op passthrough: collects all keys from World into the VisibilitySet
- Wired into `Engine::render_frame` as the first stage after `drain_changes()`
- Interface has placeholder parameters for AABB buffer + HZB texture (optional, unused in no-op)
- Unit tests for no-op producing complete visibility
- `cargo test` and `cargo clippy` pass

## Flow

### 1. New file `crates/engine/src/cull.rs`

```rust
pub struct VisibilitySet {
    pub volumes: Vec<VolumeKey>,
    pub mesh_instances: Vec<MeshInstanceKey>,
    pub lights: Vec<LightKey>,
}

pub struct CullStage;

impl CullStage {
    pub fn new() -> Self { Self }

    pub fn cull(
        &self,
        world: &World,
        view: &ViewState,
        hzb: Option<&wgpu::TextureView>,
    ) -> VisibilitySet { ... }
}
```

The `view` parameter provides camera position/orientation/FOV. `hzb` is the hierarchical-Z buffer from the previous frame (None until the HZB builder lands in sw-577c00).

No-op implementation: iterate all volumes, mesh instances, and lights in World, collect their keys.

### 2. Engine integration — `crates/engine/src/engine.rs`

- Engine owns a `CullStage` (created in `Engine::new` and `Engine::headless`)
- `render_frame` calls `cull_stage.cull(world, &self.view, None)` after `drain_changes()`
- The resulting `VisibilitySet` is stored for downstream stages (currently unused beyond this issue)

### 3. Module registration — `crates/engine/src/lib.rs`

- Add `pub mod cull;`

### 4. Tests — in `cull.rs` `#[cfg(test)]`

- Empty world → empty VisibilitySet
- World with volumes/mesh instances/lights → all keys present in VisibilitySet
- VisibilitySet counts match World counts

## Decisions

- **Struct, not trait** — no need for polymorphism. Real culling replaces the no-op body, doesn't need a separate type. If we later want swappable strategies (CPU frustum vs GPU HZB), we can add a trait then.
- **`Vec<Key>` in VisibilitySet, not `HashSet`** — downstream stages iterate linearly, never query membership. Vec is cache-friendly and allocation-free after warmup (capacity stabilizes).
- **HZB as `Option<&TextureView>`** — the HZB texture doesn't exist until sw-577c00. Passing None skips occlusion culling. Clean evolution: when the HZB builder lands, Engine passes `Some(&hzb_view)` and the cull stage uses it.
- **Lights in VisibilitySet** — even though lights re-pack every frame, the pipeline should still cull off-screen point/spot lights. Directional lights are always visible. The no-op includes all lights; real culling will filter by range/frustum.
