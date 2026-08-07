## What was built

The Stream stage — second stage of the OOC pipeline. Bridges the gap between World data and GPU-ready buffers. Volumes get async mesh extraction on the job pool; mesh assets get uploaded from CPU data. Never blocks the frame.

Informed by research into UE5 (Nanite), Unity HDRP, and Godot 4 geometry streaming architectures. Key findings documented in `docs/research/geometry-streaming.md`.

## How the pieces fit together

### `stream.rs`

**Core types:**
- `MeshData` — CPU-side vertices + indices, output of extraction
- `GpuMesh` — GPU vertex + index buffers with `byte_size` for budget tracking. `GpuMesh::upload()` creates buffers with `mapped_at_creation` for zero-copy upload.

**MeshExtractor trait + PlaceholderExtractor:**
- `MeshExtractor` is `Send + Sync`, takes `VolumeKey` + `AABB` (not `&dyn VoxelVolume` — can't send GPU resources to worker threads). Stored as `Arc<dyn MeshExtractor>` so it can be cloned into job closures.
- `PlaceholderExtractor` generates a 24-vertex / 36-index box from the AABB. Visible geometry for any volume without voxel data access.

**MeshCache:**
- Two maps: `volume_meshes: HashMap<VolumeKey, VolumeCacheEntry>` (async extracted, evictable) and `mesh_assets: HashMap<MeshKey, GpuMesh>` (uploaded from Mesh data).
- Tracks `total_bytes` against a `budget_bytes` cap (default 256 MB).
- `evict_over_budget()` removes the farthest volume mesh first (priority-based, not frame-count). Matches UE5's screen-space priority approach rather than naive time-based eviction.

**StreamStage:**
- Owns the cache, extractor (`Arc`), pending set (`HashSet<VolumeKey>`), and ready queue.
- `stream()` per-frame flow:
  1. Drain completed extraction jobs from `JobPool` into ready queue
  2. Upload from ready queue up to per-frame budget (default 8 MB) — excess stays for next frame
  3. Submit extraction jobs for uncached visible volumes, sorted by distance (closest first)
  4. Upload mesh asset GPU buffers for visible mesh instances (within budget)
  5. Evict if over memory budget
  6. Build `StreamOutput` with references to cached `GpuMesh` entries

**StreamOutput:**
- `volume_meshes: Vec<(VolumeKey, &GpuMesh)>` — visible volumes with cached GPU buffers
- `mesh_instances: Vec<(MeshInstanceKey, &GpuMesh)>` — visible instances with cached mesh asset buffers
- `lights: Vec<LightKey>` — passed through from visibility set
- Zero-copy: borrows from cache, no cloning

### Engine integration

`Engine` owns a `StreamStage` with `PlaceholderExtractor`. `render_frame` calls `stream_stage.stream(...)` after cull, passing world, visibility, job pool, device, and camera position. Pipeline flow is now: `drain_changes` → `cull` → `stream` → placeholder render.

### Research documentation

Two research reports saved to `docs/research/`:
- `scene-architecture.md` — UE5/Unity/Godot scene containers, dirty tracking, material model (from sw-2985d8)
- `geometry-streaming.md` — async mesh prep, GPU upload, caching, eviction, streaming triggers (from this issue)

## Key decisions

- **Upload budget (8 MB/frame)** — prevents frame spikes when many extractions complete simultaneously. Ready queue buffers excess for subsequent frames. Inspired by Unity's `asyncUploadTimeSlice`.
- **Priority ordering by camera distance** — closest volumes submitted first so the most visually impactful geometry completes first. Matches UE5's streaming priority.
- **Priority-based eviction with memory budget** — evict farthest entries when over 256 MB. No production engine uses pure frame-count eviction. UE5 uses screen-space priority.
- **`Arc<dyn MeshExtractor>` not `Box`** — enables safe cloning into job closures without unsafe raw pointer casts.
- **`stream()` is `pub(crate)`** — only Engine calls it. Avoids exposing `JobPool` (which is `pub(crate)`) in a public API.

## What to know for future work

- `PlaceholderExtractor` generates AABB boxes. Real extractors will access CPU-side voxel data via `Arc` state and produce actual triangle meshes.
- GPU buffer pooling / suballocation tracked in sw-f53904.
- Pipeline profiling tracked in sw-c162ed.
- Always-resident coarse LOD (UE5's root clusters pattern) not yet filed — future E2/E3 work.
- The `StreamOutput` lifetime is tied to `&mut self` on `StreamStage`, which means the output can't outlive the stream call. Execute stages will consume it within the same `render_frame` scope.
