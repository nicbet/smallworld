## What was built

The Cull stage — the first stage of the OOC pipeline. It takes the World and view parameters, produces a `VisibilitySet` containing the keys of everything that needs rendering this frame. Wired into `Engine::render_frame` so the pipeline flow is now: drain_changes → **cull** → (stream, execute come later).

## How the pieces fit together

### `cull.rs`

**`VisibilitySet`** — output struct with three `Vec<Key>` fields: `volumes`, `mesh_instances`, `lights`. Has `total()` and `is_empty()` helpers. Downstream stages iterate these vecs linearly — Vec is cache-friendly and allocation-free after the first frame (capacity stabilizes).

**`CullStage`** — unit struct, `#[derive(Default)]`. The `cull()` method takes:
- `&World` — iterates all volumes, mesh instances, and lights
- `&ViewState` — camera position/orientation/FOV (unused in no-op, ready for frustum culling)
- `Option<&wgpu::TextureView>` — HZB texture from previous frame (None until sw-577c00)

Current implementation is a no-op passthrough: collects all keys from World into the VisibilitySet. When real culling lands, it replaces this body — frustum test against `VoxelVolume::bounds()` / mesh instance AABBs, then HZB occlusion for GPU-side rejection.

### Engine integration

`Engine` owns a `CullStage` field, created in both `new()` and `headless()`. In `render_frame`, cull runs immediately after `drain_changes()`:

```
drain_changes → cull(world, view, None) → [placeholder render]
```

The `VisibilitySet` is currently `_visibility` (unused) — the Stream stage (sw-59c80a) will consume it.

## Key decisions

- **Struct, not trait** — no polymorphism needed. Real culling replaces the method body, doesn't need a separate type. A trait can be introduced later if swappable strategies (CPU frustum vs GPU HZB) are needed.
- **`Vec<Key>`, not `HashSet`** — downstream stages iterate linearly, never query membership. Vec is faster for iteration and avoids hashing overhead.
- **HZB as `Option<&TextureView>`** — clean evolution path. Engine passes `None` now, `Some(&hzb_view)` when the HZB builder lands in sw-577c00.
- **Lights included in VisibilitySet** — even though lights re-pack every frame, the pipeline should still cull off-screen point/spot lights in the future. The no-op includes all lights.

## What to know for future work

- Real frustum culling needs view + projection matrices. `FreeCamera` already has `view_matrix()` and `projection_matrix()` — extract 6 frustum planes from the VP matrix.
- HZB occlusion culling: after the HZB builder lands (sw-577c00), project AABBs to screen space and test against the mip chain.
- SSE evaluation: `VoxelVolume::lod_hint()` provides `voxel_scale` and `max_depth` for screen-space error calculations.
