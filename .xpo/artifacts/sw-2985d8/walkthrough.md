## What was built

World became the engine's hybrid scene container — holding everything the pipeline renders across two rendering paths that converge at the GBuffer:

- **Voxel volumes** (`SlotMap<VolumeKey, Box<dyn VoxelVolume>>`) — the raymarched world (terrain, structures, environment)
- **Mesh assets** (`SlotMap<MeshKey, Mesh>`) — shared vertex/index geometry
- **Mesh instances** (`SlotMap<MeshInstanceKey, MeshInstance>`) — placed objects referencing a `MeshKey` + `MaterialKey`
- **Materials** (`SlotMap<MaterialKey, Material>`) — shared PBR properties (base_color, roughness, metallic, emissive)
- **Lights** (`SlotMap<LightKey, Light>`) — directional, point, and spot with no change tracking

The old `VoxelModel` + `VoxelInstance` system was removed from World. It was the raymarcher's internal data model, not a game-facing concept.

## How the pieces fit together

### New files

- `light.rs` — `LightKind` enum (Directional/Point/Spot) with spatial parameters per variant. `Light` struct with shared fields (color, intensity, shadow flag). Convenience constructors: `Light::directional()`, `Light::point()`, `Light::spot()`.

- `material.rs` — `Material` struct with scalar PBR properties. Shared via `MaterialKey` — multiple mesh instances reference the same material. Texture maps are future scope (sw-8d894c).

- `mesh.rs` — `Vertex` is `#[repr(C)]` with position, normal, UV, tangent (w = bitangent handedness). `Mesh` holds vertex/index vecs + auto-computed AABB. `MeshInstance` references `MeshKey` + `MaterialKey` + transform + shadow flag.

### World rewrite (`world.rs`)

Five `SlotMap` collections with full CRUD: `add_*`, `remove_*`, `*()` (get), `*_mut()` (get mut), iterator, count.

**Per-object change tracking** via `ChangeSet<K>` (spawned/despawned/mutated vecs). Each mutating method records the affected key. `drain_changes()` returns a `WorldChanges` struct and clears all pending changes — called by Engine once per frame. This matches UE5's `MarkRenderStateDirty` / Godot's per-instance dirty list pattern: adding one entity doesn't re-upload all geometry.

**Lights are excluded from change tracking.** They're re-packed into a small GPU buffer every frame (a few KB for dozens of lights). This matches all three major engines — light data is cheap to rebuild; geometry extraction is expensive.

**`mesh_instance_mut()` records mutated** — transform/material-ref changes need GPU buffer patches. **`light_mut()` does not** — lights re-pack every frame regardless.

### Engine integration (`engine.rs`)

`render_frame` calls `world.drain_changes()` at the top of the frame. The result is currently discarded — pipeline stages (Cull, Stream, Execute) will consume it in subsequent issues. The old `WorldGpuData::extract` path was removed. The placeholder renderer continues working since it doesn't read World data.

### Sandbox (`main.rs`)

`populate_test_scene()` exercises the full game-facing API: a directional sun + point fill light, a stone material, a floor quad mesh, and a mesh instance placing it at origin. The scene data flows through `drain_changes()` every frame — renderable as soon as the GBuffer lands.

## Key decisions

- **Hybrid from day one** — voxel-first but not voxel-only, matching UE5/Unity/Godot. This was a design conversation with the user, informed by researching all three engines' scene architectures.

- **Per-object dirty tracking over global dirty flag** — all three major engines use per-object granularity. A single `geometry_dirty: bool` would cause a flickering torch to trigger BVH rebuilds. The `ChangeSet<K>` pattern was chosen after comparing UE5 (per-component `MarkRenderStateDirty`), Unity (per-renderer), and Godot (per-instance dirty list with granular flags).

- **Materials shared via reference** — all three engines share materials via handle, not embedded per-instance. Even though our raymarcher treats material as data (not a shader program), sharing avoids per-instance copies and enables batch-friendly rendering.

- **`Mesh` + `MeshInstance` naming** — follows Godot convention. Immediately communicates the asset/placement split. Discussed and confirmed with user.

- **`WorldGpuData` kept as stub** — the struct and its buffer accessors remain for the raymarcher. `extract()` returns empty. Pipeline stages will repopulate it.

## What to know for future work

- `voxel_object.rs` still exists with `VoxelModel`/`VoxelInstance`/`VoxelInstanceGpu` — used by the old sandbox. Can be removed when the old sandbox is retired.
- Material textures tracked in sw-8d894c (BACKLOG), depends on GBuffer (sw-577c00).
- `DrainedChanges`/`WorldChanges` are `#[allow(dead_code)]` until pipeline stages consume them.
- Duplicate keys in `mutated` vec are possible if `*_mut()` is called twice on the same key in one frame. Pipeline stages should process idempotently.
