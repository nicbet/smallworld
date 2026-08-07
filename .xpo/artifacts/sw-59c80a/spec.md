## What

Second stage of the OOC pipeline. Takes the `VisibilitySet` from Cull, ensures visible geometry has GPU buffers ready for Execute. Volumes get async mesh extraction on the job pool; mesh instances get their shared `Mesh` data uploaded. Never blocks the frame — renders whatever's already cached.

Includes upload budget (cap uploads per frame), priority ordering (closer/larger first), and priority-based eviction with memory budget — informed by UE5/Unity/Godot research.

## Why

Execute needs GPU vertex/index buffers to draw. Volumes store voxel data, not triangles — they need mesh extraction (voxels → triangles) which is expensive and must run async. Mesh assets have CPU-side vertex/index data that needs GPU upload. The Stream stage bridges the gap between World data and renderable GPU buffers.

## Acceptance Criteria

### Core types
- `MeshData` — CPU-side extraction result (vertices + indices)
- `GpuMesh` — GPU vertex + index buffers with index count and byte size
- `MeshExtractor` trait: `extract(key, bounds) -> MeshData`, `Send + Sync`
- `PlaceholderExtractor` — generates a box from the volume's AABB

### Cache
- `MeshCache` — caches `GpuMesh` for volumes and mesh assets
- Priority-based eviction: when cache exceeds memory budget, evict lowest-priority entries (farthest from camera, smallest screen-space contribution)
- Tracks total GPU bytes allocated

### Streaming
- `StreamStage` receives VisibilitySet, submits extraction jobs for uncached visible volumes
- Priority ordering: closer/larger volumes submitted first
- Upload budget: cap GPU uploads per frame (byte limit), excess queued for next frame
- Drain completed extraction jobs each frame
- Mesh instance GPU upload: visible `MeshKey`s not yet cached get uploaded (within budget)
- Pending set prevents duplicate job submission

### Integration
- Wired into `Engine::render_frame` after cull
- `StreamOutput` provides key→GpuMesh mapping for Execute
- Unit tests for cache, extraction, eviction, budget
- `cargo test` and `cargo clippy` pass

## Flow

### 1. New file `crates/engine/src/stream.rs`

**MeshData + GpuMesh:**

```rust
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub byte_size: u64,
}
```

`byte_size` tracks GPU memory for the budget system.

**MeshExtractor trait:**

```rust
pub trait MeshExtractor: Send + Sync {
    fn extract(&self, key: VolumeKey, bounds: AABB) -> MeshData;
}
```

Runs on worker threads. Takes key + AABB, not `&dyn VoxelVolume` (can't send GPU resources to workers). Real extractors access CPU-side voxel data via `Arc` state.

**PlaceholderExtractor:**

Generates a box mesh from the AABB — 24 vertices (with normals), 36 indices. Visible geometry for any volume without voxel data access.

**MeshCache:**

```rust
struct CacheEntry {
    gpu_mesh: GpuMesh,
    last_camera_distance: f32,
}

pub struct MeshCache {
    volume_meshes: HashMap<VolumeKey, CacheEntry>,
    mesh_assets: HashMap<MeshKey, GpuMesh>,
    total_bytes: u64,
    budget_bytes: u64,
}
```

Eviction: when `total_bytes > budget_bytes`, sort entries by `last_camera_distance` descending (farthest first), evict until under budget. Default budget: 256 MB.

**ReadyQueue — upload budget:**

```rust
struct ReadyEntry {
    key: VolumeKey,
    mesh_data: MeshData,
}

pub struct StreamStage {
    cache: MeshCache,
    extractor: Box<dyn MeshExtractor>,
    pending: HashSet<VolumeKey>,
    ready_queue: Vec<ReadyEntry>,
    upload_budget_bytes: u64,  // per-frame cap, default 8 MB
}
```

Completed extraction jobs drain into `ready_queue`. Each frame, upload up to `upload_budget_bytes` from the queue. Excess stays for next frame. Converts worst-case spikes into amortized cost.

**stream() method:**

1. Drain completed extraction jobs from JobPool into ready_queue
2. Upload from ready_queue up to budget → insert into cache
3. For each visible volume (sorted by distance, closest first):
   - If cached: update `last_camera_distance`
   - If not cached and not pending and not in ready_queue: submit extraction job
4. For each visible mesh instance: if its MeshKey not cached, upload from Mesh data (within budget)
5. Evict if over memory budget
6. Return `StreamOutput`

**Priority ordering for job submission:**

Sort visible uncached volumes by projected screen-space size (AABB extent / distance). Submit largest-on-screen first. JobPool processes them in order so the most impactful volumes complete first.

### 2. StreamOutput

```rust
pub struct StreamOutput<'a> {
    pub volume_meshes: Vec<(VolumeKey, &'a GpuMesh)>,
    pub mesh_instances: Vec<(MeshInstanceKey, &'a GpuMesh)>,
}
```

Only entries with cached GPU buffers. Newly visible volumes without cached meshes are absent until extraction completes.

### 3. Engine integration

- Engine creates `StreamStage` with `PlaceholderExtractor`
- `render_frame` calls `stream_stage.stream(...)` after cull, passing world, visibility, job pool, device, and camera position
- `JobPool` field loses `#[allow(dead_code)]`

### 4. Module registration

- Add `pub mod stream;` to `lib.rs`

### 5. Tests

- `PlaceholderExtractor` produces correct vertex/index counts for a box
- `MeshCache` insert/get/eviction under budget
- `MeshCache` eviction removes farthest entries first
- Ready queue respects upload budget (partial drain)
- Priority ordering: closer volumes sorted first
- Empty VisibilitySet → empty StreamOutput

## Decisions

- **Upload budget (8 MB/frame default)** — prevents frame spikes when many extractions complete at once. Matches Unity's `asyncUploadTimeSlice` concept. Excess stays in ready_queue for next frame.
- **Priority ordering by screen-space size** — most visually impactful volumes complete first. Matches UE5's streaming priority. Cheap to compute: `aabb_extent / distance_to_camera`.
- **Priority-based eviction with memory budget** — evict farthest/smallest entries when over budget. Better than frame-count eviction (which evicts based on time, not importance). Default 256 MB budget.
- **`MeshExtractor` takes key + AABB, not `&dyn VoxelVolume`** — extraction runs on worker threads. `VoxelVolume` may hold GPU resources and may not be Send.
- **Two separate cache maps** — different lifecycles. Volume meshes are extracted async. Mesh asset buffers are uploaded from existing CPU data.
- **`PlaceholderExtractor` generates a box** — visible geometry without voxel data access. Better than nothing (Unity/Godot show nothing until loaded). Coarse LOD upgrade tracked separately.
- **Pending set prevents duplicate jobs** — matches UE5's in-flight request tracking.
- **`byte_size` on GpuMesh** — enables accurate memory budget tracking without querying wgpu.
- **Per-volume GPU buffers (for now)** — suballocation/pooling is follow-up sw-f53904.

## Edge Cases

- **Volume removed while extraction in-flight**: job completes, key no longer in World. Discard the result on drain. (LOW)
- **Ready queue grows unbounded during loading burst**: bounded by pending set (one entry per volume). In practice, limited by world volume count. (LOW)
- **Upload budget too small for one mesh**: if a single MeshData exceeds the budget, upload it anyway (don't deadlock). Log a warning. (LOW)
- **Zero visible volumes/meshes**: stream() returns empty StreamOutput. (LOW)

## Assumptions

- Existing `JobPool` API sufficient — no changes needed.
- `PlaceholderExtractor` is temporary. Real extractors are volume-type-specific.
- Mesh asset upload is within budget alongside volume uploads — shared budget pool.
- GPU buffer pooling deferred to sw-f53904.
