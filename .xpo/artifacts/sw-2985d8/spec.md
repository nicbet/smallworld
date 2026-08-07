## What

World becomes the hybrid container for everything the engine renders: voxel volumes (raymarched world), mesh instances (rasterized near-field objects), shared materials, and lights (shared illumination). Per-object change tracking lets the pipeline process only what changed. The APIs are complete and correct from day one — rendering comes in later issues.

## Why

The OOC pipeline needs input data. The old `VoxelModel` + `VoxelInstance` system was the raymarcher's internal data model, not a game-facing concept. World needs to express what games actually place in a scene: voxel worlds, mesh objects, and lights. Best-in-class engines (UE5, Unity HDRP, Godot 4) are all hybrid — raytracing/compute for mid-far world and lighting, rasterization for near-field animated objects. Both paths converge at the GBuffer. All three engines use per-object dirty tracking and shared materials.

## Acceptance Criteria

### Volumes
- `World::add_volume(Box<dyn VoxelVolume>) -> VolumeKey`
- `World::remove_volume(VolumeKey) -> Option<Box<dyn VoxelVolume>>`
- `World::volume(VolumeKey) -> Option<&dyn VoxelVolume>`
- `World::volumes()` iterates `(VolumeKey, &dyn VoxelVolume)`
- `World::volume_count() -> usize`
- Add records spawned in `ChangeSet<VolumeKey>`; remove records despawned

### Materials (shared assets)
- `World::add_material(Material) -> MaterialKey`
- `World::remove_material(MaterialKey) -> Option<Material>`
- `World::material(MaterialKey) -> Option<&Material>`
- `World::material_mut(MaterialKey) -> Option<&mut Material>` (records mutated)
- `World::materials()` iterates all

### Meshes (shared geometry assets)
- `World::add_mesh(Mesh) -> MeshKey`
- `World::remove_mesh(MeshKey) -> Option<Mesh>`
- `World::mesh(MeshKey) -> Option<&Mesh>`
- `World::meshes()` iterates all

### Mesh Instances (placed objects)
- `World::add_mesh_instance(MeshInstance) -> MeshInstanceKey`
- `World::remove_mesh_instance(MeshInstanceKey) -> Option<MeshInstance>`
- `World::mesh_instance(key) -> Option<&MeshInstance>`
- `World::mesh_instance_mut(key) -> Option<&mut MeshInstance>` (records mutated)
- `World::mesh_instances()` iterates all
- `MeshInstance` references `MeshKey` + `MaterialKey`

### Lights
- `World::add_light(Light) -> LightKey`
- `World::remove_light(LightKey) -> Option<Light>`
- `World::light(LightKey) -> Option<&Light>`
- `World::light_mut(LightKey) -> Option<&mut Light>` (no change tracking — re-packed per frame)
- `World::lights()` iterates `(LightKey, &Light)`
- `World::light_count() -> usize`

### Change Tracking
- `ChangeSet<K>` tracks spawned, despawned, and mutated keys per collection
- `World::drain_changes()` returns all pending changes and clears them (called by Engine once per frame)
- Lights excluded from change tracking — re-packed every frame (a few KB for dozens of lights)

### Cleanup
- `VoxelModel` and `VoxelInstance` removed from World
- `WorldGpuData::extract` stubbed (no models/instances to extract)
- Old sandbox (`sandbox_old`) unaffected

### General
- Unit tests for all CRUD operations and change tracking
- `cargo test` and `cargo clippy` pass

## Flow

### 1. Change tracking — `crates/engine/src/world.rs` (internal type)

```rust
pub(crate) struct ChangeSet<K: slotmap::Key> {
    spawned: Vec<K>,
    despawned: Vec<K>,
    mutated: Vec<K>,
}
```

Methods: `mark_spawned`, `mark_despawned`, `mark_mutated`, `is_empty`, `drain` (returns owned vecs + clears). World mutation methods populate these automatically. Engine drains them once per frame before pipeline stages run.

Per-object granularity means adding one volume doesn't re-upload all geometry. Pipeline stages process only the spawned/despawned/mutated keys.

### 2. Light types — new file `crates/engine/src/light.rs`

```rust
pub enum LightKind {
    Directional {
        direction: Vec3,
    },
    Point {
        position: Vec3,
        range: f32,
    },
    Spot {
        position: Vec3,
        direction: Vec3,
        range: f32,
        inner_angle: f32,  // half-angle, full brightness cone (radians)
        outer_angle: f32,  // half-angle, falloff-to-zero cone (radians)
    },
}

pub struct Light {
    pub kind: LightKind,
    pub color: Vec3,
    pub intensity: f32,
    pub casts_shadows: bool,
}
```

No change tracking for lights. The pipeline reads `world.lights()` every frame and packs a small GPU SSBO. Matches UE5/Unity/Godot — light data is cheap to rebuild.

### 3. Material type — new file `crates/engine/src/material.rs`

```rust
pub struct Material {
    pub base_color: Vec4,   // RGB + alpha
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: Vec3,
}
```

Shared via `MaterialKey`. Multiple mesh instances reference the same material. Changing a material's properties (via `material_mut`) records it as mutated so the pipeline can re-upload just that material's GPU data.

### 4. Mesh types — new file `crates/engine/src/mesh.rs`

```rust
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],  // xyz = tangent, w = bitangent handedness
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub bounds: AABB,
}

pub struct MeshInstance {
    pub mesh: MeshKey,
    pub material: MaterialKey,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub casts_shadows: bool,
}
```

### 5. World rewrite — `crates/engine/src/world.rs`

Remove `VoxelModel`, `VoxelInstance`, `InstanceKey`. New shape:

```rust
pub struct World {
    // Voxel world — raymarched
    volumes: SlotMap<VolumeKey, Box<dyn VoxelVolume>>,
    volume_changes: ChangeSet<VolumeKey>,

    // Mesh objects — rasterized
    meshes: SlotMap<MeshKey, Mesh>,
    mesh_changes: ChangeSet<MeshKey>,
    mesh_instances: SlotMap<MeshInstanceKey, MeshInstance>,
    mesh_instance_changes: ChangeSet<MeshInstanceKey>,

    // Materials — shared
    materials: SlotMap<MaterialKey, Material>,
    material_changes: ChangeSet<MaterialKey>,

    // Lighting — no change tracking, re-packed per frame
    lights: SlotMap<LightKey, Light>,
}
```

`drain_changes()` returns a `WorldChanges` struct with all four change sets drained, called by Engine once per frame.

### 6. WorldGpuData

`WorldGpuData::extract` currently depends on `VoxelModel`/`VoxelInstance`. With those removed, extract becomes a stub returning `WorldGpuData::empty()`. The struct and accessors stay — pipeline stages will repopulate them.

### 7. Engine plumbing — `crates/engine/src/engine.rs`

- `render_frame` calls `world.drain_changes()` (discards result for now — pipeline stages consume it later)
- Placeholder renderer continues working (doesn't read World data)

### 8. Module registration — `crates/engine/src/lib.rs`

- Add `pub mod light;`
- Add `pub mod material;`
- Add `pub mod mesh;`

### 9. Sandbox update — `crates/sandbox/src/main.rs`

- Remove any VoxelModel/VoxelInstance usage if present

### 10. Tests — `world.rs` `#[cfg(test)]` module

- Volume: add/remove, change set records spawned/despawned
- Material: add/remove/mutate, change set records spawned/despawned/mutated
- Mesh: add/remove
- MeshInstance: add/remove/mutate, change set records correctly
- Light: add/remove, `light_mut` does NOT populate any change set
- `drain_changes` clears all pending changes
- Trait object coercion for volumes (dummy VoxelVolume impl)

## Decisions

- **Hybrid from day one** — voxel volumes + mesh instances + lights. Matches UE5/Unity/Godot architecture. Both paths converge at the GBuffer.
- **Remove VoxelModel/VoxelInstance** — old raymarcher's internal data model. Volumes replace the voxel side; Mesh/MeshInstance replace the object side.
- **Per-object change tracking via `ChangeSet<K>`** — matches UE5 (`MarkRenderStateDirty` per component), Godot (per-instance dirty list). Adding one entity doesn't re-upload all geometry. Pipeline stages process only changed keys.
- **Lights excluded from change tracking** — re-packed every frame like UE5/Unity/Godot. A few KB for dozens of lights.
- **Materials shared via `MaterialKey`** — matches all three engines. Multiple instances reference one material. Avoids embedding copies per instance, enables batch-friendly rendering, and changing one material updates all referencing instances.
- **`Mesh` + `MeshInstance` naming** — follows Godot convention. Immediately communicates asset/placement split.
- **SlotMap for all collections** — stable handles across frames, O(1) insert/remove. Meshes and materials need removal for level streaming.
- **PBR vertex format** — position + normal + UV + tangent. Standard baseline across all three engines. Additional vertex attributes (color, multiple UV channels) are future scope.
- **Spot light half-angles in radians** — inner (full brightness) and outer (falloff-to-zero). Standard in UE/Unity/Godot.
- **`mesh_instance_mut` records mutated** — transform/material-ref changes need GPU buffer patches. Unlike lights (re-packed every frame), instance transforms live in a large structured buffer where incremental updates matter.

## Edge Cases

- **MeshInstance referencing removed Mesh or Material** — dangling `MeshKey`/`MaterialKey`. SlotMap returns `None` on lookup, so the renderer skips the instance. (LOW — document that removing an asset invalidates referencing instances)
- **Empty world**: valid state. All iterators return empty. `drain_changes` returns empty changes. (LOW)
- **Volume with GPU resources dropped on remove**: wgpu buffers are ref-counted, safe. (LOW)
- **Duplicate mutated entries in ChangeSet** — if `mesh_instance_mut` is called twice on the same key in one frame, the key appears twice in `mutated`. Pipeline stages should handle duplicates (deduplicate or process idempotently). (LOW — process idempotently)

## Assumptions

- Old sandbox (`sandbox_old`) manages its own data directly, doesn't use the engine's World.
- `VoxelVolume` trait objects may hold GPU resources. World doesn't manage their lifecycle beyond Box ownership.
- Skinned mesh support (bone weights, skeleton) is future scope — MeshInstance is for static and rigid-body meshes.
- Material textures are future scope — the Material struct covers scalar PBR properties only.
