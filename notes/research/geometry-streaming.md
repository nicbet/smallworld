# Geometry Streaming: UE5, Unity HDRP, Godot 4

*Research date: 2026-08-08 — conducted for sw-59c80a (Stream stage: async mesh + upload pipeline)*

## 1. Async Mesh Preparation

**UE5 (Nanite):** Three-thread pipeline (Game → Render → RHI). Nanite streaming is GPU-driven — the GPU walks the cluster DAG, determines which pages are needed, and writes requests to a feedback buffer. CPU-side `NaniteStreamingManager` decompresses cluster pages on worker threads and hands data to the RHI thread for upload. The GPU never waits on the CPU; it renders whatever clusters are already resident.

**Unity (HDRP/URP):** Two mechanisms. Procedural meshes use `Mesh.AllocateWritableMeshData` (writable from any thread via Job System + Burst), but `Mesh.ApplyAndDisposeWritableMeshData` must be called on the main thread — hard sync point. Asset loading uses the Async Upload Pipeline. DOTS subscenes use `SceneSystem.LoadSceneAsync`.

**Godot 4:** `ResourceLoader.load_threaded_request` handles async resource loading on worker threads. Mesh GPU buffer creation happens on the render thread when the resource is committed. No equivalent of Unity's `MeshDataArray` for building mesh data from worker threads.

## 2. GPU Buffer Management

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Upload mechanism | RHI thread into streaming pool | Time-sliced ring buffer | Synchronous on render thread |
| Buffer allocation | Pooled streaming pool, suballocated | Per-mesh + ring buffer staging | Per-mesh individual buffers (VMA) |
| Upload budget | Implicit (RHI thread pipelining) | `asyncUploadTimeSlice` (ms/frame) | None |
| Staging | GPU transcoding via compute shader | Single ring buffer (2-2047 MB) | Per-frame staging buffer pool |

**UE5** uses a fixed-size streaming pool (`r.Nanite.Streaming.StreamingPoolSize` MB), allocated as reserved virtual memory. Pages are decompressed on CPU workers, then GPU-transcoded via compute shader.

**Unity** uses `asyncUploadBufferSize` (2 MB to 2047 MB) ring buffer with `asyncUploadTimeSlice` (ms) budgeting how much CPU time per frame for uploads. Time-sliced once per render frame (twice during async loads).

**Godot 4** uses per-mesh GPU buffer allocation via VMA. Each `buffer_update()` uses staging internally. Transfer Workers (4.3+) get their own staging buffer + command buffer on a dedicated transfer queue.

## 3. Mesh/Geometry Caching

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Cache unit | Cluster page (128 KB) | None (mesh stays resident) | None (resource ref-counted) |
| Eviction | Priority by screen-space contribution, memory budget | None | None |
| Always-resident fallback | Root clusters (never evicted) | No | No |
| Memory budget | `StreamingPoolSize` (developer-set) | None for meshes | None |

**UE5's always-resident root clusters** are the most important design decision: the coarsest LOD is permanently GPU-resident, guaranteeing something always renders for every Nanite mesh. Additional detail streams in progressively.

## 4. Streaming Triggers

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Trigger | GPU screen-space error feedback | Explicit / LODGroup screen height | Explicit / distance ranges |
| Priority | Screen-space contribution | None | None |
| Automatic LOD | GPU-driven DAG traversal | LODGroup (screen-relative height) | meshoptimizer-generated, screen-size heuristic |

**UE5's GPU feedback loop:** During culling/LOD selection compute shaders, when a cluster's page is not resident, the shader writes a streaming request. The CPU reads these back and prioritizes page loading by screen-space contribution.

## 5. Handling In-Flight Work (Not Yet Ready)

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Fallback | Root clusters always render (coarse LOD) | Nothing until loaded | Nothing until loaded |
| Visual result | Graceful LOD refinement | Pop-in | Pop-in |
| Placeholder system | Built-in (root clusters) | None | None (PlaceholderMesh is for dedicated servers only) |

**UE5's design is fundamentally superior here:** there is never a frame where a Nanite mesh is invisible due to streaming. Root clusters appear instantly, detail refines over subsequent frames.

## 6. Frame Blocking

| Aspect | UE5 | Unity | Godot 4 |
|--------|-----|-------|---------|
| Blocks frame? | Never | Can block (`ApplyAndDispose`, upload spikes) | Can block (sync upload, `sync()` stall) |
| Mitigation | 3-thread pipeline | `asyncUploadTimeSlice` | Transfer Workers (4.3+), rendering acyclic graph |

## Summary Table

| Aspect | UE5 (Nanite) | Unity | Godot 4 | smallworld (sw-59c80a) |
|--------|--------------|-------|---------|------------------------|
| Async extraction | Worker threads decompress pages | Job System + Burst | Worker threads via ResourceLoader | Worker threads via JobPool |
| GPU upload | RHI thread into streaming pool | Time-sliced ring buffer | Synchronous on render thread | Budgeted sync at drain time |
| Cache eviction | Priority by screen-space contribution | None | None | Priority by camera distance, memory budget |
| Always-resident fallback | Root clusters (never evicted) | No | No | AABB box placeholder |
| Frame blocking | Never | Can block | Can block | Never (by design) |
| Upload budget | Implicit (RHI thread) | asyncUploadTimeSlice (ms/frame) | None | 8 MB/frame cap |
| Buffer management | Pooled streaming pool | Per-mesh + ring buffer | Per-mesh (VMA) | Per-volume (follow-up: pooling) |
| Priority ordering | Screen-space contribution | None | None | Camera distance |

## Design Decisions Informed by This Research

1. **Upload budget (8 MB/frame)** — prevents frame spikes, matches Unity's `asyncUploadTimeSlice` concept
2. **Priority ordering by distance** — most visually impactful volumes first, matches UE5's streaming priority
3. **Priority-based eviction with memory budget** — evict farthest entries when over 256 MB budget, better than frame-count eviction
4. **Never blocks the frame** — matches UE5's non-blocking architecture
5. **`PlaceholderExtractor` generates AABB box** — better than Unity/Godot's "nothing until ready", but not as good as UE5's always-resident root clusters

## Follow-Up Issues Filed

- **sw-f53904** (BACKLOG) — GPU buffer pooling / suballocation (replace per-volume individual buffer allocations with slab allocator)
- Always-resident coarse LOD — two-tier cache with permanent root LOD (not yet filed, future E2/E3 work)

## Sources

- [Nanite SIGGRAPH 2021 — Karis, Stubbe, Wihlidal](https://advances.realtimerendering.com/s2021/Karis_Nanite_SIGGRAPH_Advances_2021_final.pdf)
- [Nanite HPG 2022 Keynote — Brian Karis](https://www.highperformancegraphics.org/slides22/Journey_to_Nanite.pdf)
- [Nanite Technical Details — UE 5.8 Docs](https://dev.epicgames.com/documentation/unreal-engine/nanite-technical-details)
- [Threaded Rendering — UE 5.8 Docs](https://dev.epicgames.com/documentation/unreal-engine/threaded-rendering-in-unreal-engine)
- [Unity Async Upload Pipeline](https://docs.unity3d.com/6000.3/Documentation/Manual/configure-asynchronous-upload-pipeline.html)
- [Unity Mesh.AllocateWritableMeshData](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Mesh.AllocateWritableMeshData.html)
- [Godot Internal Rendering Architecture](https://docs.godotengine.org/en/4.4/contributing/development/core_and_modules/internal_rendering_architecture.html)
- [Godot Rendering Acyclic Graph](https://godotengine.org/article/rendering-acyclic-graph/)
- [Godot Transfer Workers PR #87590](https://github.com/godotengine/godot/pull/87590)
- [Godot Consolidate Vertex Buffers Proposal #11620](https://github.com/godotengine/godot-proposals/issues/11620)
