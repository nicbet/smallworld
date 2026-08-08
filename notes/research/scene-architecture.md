# Scene Architecture: UE5, Unity HDRP, Godot 4

*Research date: 2026-08-07 — conducted for sw-2985d8 (World accepts volumes and lights)*

## 1. World / Scene Container

| Aspect | UE5 | Unity HDRP/URP | Godot 4 |
|--------|-----|----------------|---------|
| Top container | `FScene` (render thread) | `CullingResults` (per-frame) | `Scenario` |
| Geometry storage | Primitive array + octree | Renderer list from culling | `DynamicBVH[INDEXER_GEOMETRY]` + `PagedArray<InstanceData>` |
| Light storage | `TSparseArray<FLightSceneInfoCompact>` (separate) | `NativeArray<VisibleLight>` (separate) | `DynamicBVH[INDEXER_VOLUMES]` + directional_lights list (separate) |
| Spatial acceleration | Octree | Frustum + occlusion culling (engine-internal) | Two DynamicBVH trees |

All three engines store lights separately from geometry.

## 2. Mesh/Geometry Asset vs Instance

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Shared asset | `UStaticMesh` | `Mesh` (via `sharedMesh`) | `RID` from `mesh_create()` |
| Placed instance | `UStaticMeshComponent` / `FPrimitiveSceneProxy` | `MeshRenderer` + `MeshFilter` | `Instance` struct (holds `RID base`) |
| Relationship | Component references asset; proxy mirrors on render thread | Filter references shared mesh; renderer draws it | Instance references resource by RID |
| Bulk instancing | `UInstancedStaticMeshComponent` | `BatchRendererGroup` / `RenderMesh` (DOTS) | `MultiMesh` / `multimesh_create()` |

Universal pattern: shared geometry asset + placed instance with transform.

## 3. Light Storage and Management

All three engines store lights in separate typed collections from geometry:

- **UE5**: `FScene::Lights` as `TSparseArray<FLightSceneInfoCompact>`. Added/removed via separate `AddLight()`/`RemoveLight()` paths from primitives.
- **Unity**: `CullingResults.visibleLights` is a `NativeArray<VisibleLight>` populated after culling. Light data uploaded to GPU as structured buffers.
- **Godot 4**: Lights are instances with `base_type` set to a light type, stored in `DynamicBVH[INDEXER_VOLUMES]` plus explicit directional/dynamic lists.

## 4. Dirty Tracking

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Granularity | Per-component (`MarkRenderStateDirty`) | Per-renderer | Per-instance (dirty list with granular flags) |
| Light change → geometry dirty? | No | No | No (separate flags) |
| Transform/material change → geometry dirty? | No (uniform buffer update) | No (SRP batcher handles) | No (per-instance flag) |
| Structural change (add/remove) | Yes (re-registration) | Yes (re-culling) | Yes (BVH update) |

**Key finding:** All three engines use per-object dirty tracking, not a single global boolean. Light property changes never trigger geometry re-extraction.

## 5. Material Model

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Model | Shared via handle (`UMaterial`/`UMaterialInstance`) | Shared via reference (`sharedMaterial`) | Shared via RID |
| Per-instance override | `SetMaterial()` replaces slot reference | `.material` clones, or `MaterialPropertyBlock` | `material_override` RID |

All three engines share materials via handle/reference, not embedded per-instance.

## 6. Voxel Volumes as First-Class Primitive

No production engine has `VoxelVolume` as a core scene primitive. UE5's Nanite is virtualized polygon geometry (not voxels). Unity has no native voxel representation. Godot's `VoxelGI` is a lighting technique, not geometry. This is smallworld's differentiator.

## Design Decisions Informed by This Research

1. **Per-object change tracking** via `ChangeSet<K>` — matches UE5/Unity/Godot granularity
2. **Lights excluded from change tracking** — re-packed every frame (all three engines do this)
3. **Materials shared via `MaterialKey`** — matches universal shared-material pattern
4. **Hybrid World** (volumes + meshes + materials + lights) — matches UE5/Unity/Godot architecture
5. **`Mesh` + `MeshInstance` naming** — follows Godot convention
