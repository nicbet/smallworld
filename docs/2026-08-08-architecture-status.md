# Architecture status

Gap analysis of `smallworld-architecture.md` against the codebase.
Generated 2026-08-09.

**Legend:** Exists = implemented and aligned. Incomplete = interface or partial code exists. Absent = no code.

**Issue tracking:** The build-order items below map to stories in **E1: Core Abstractions
& Pipeline Foundation** (sw-449c64). Step labels (A.1, B.2, 4a.4, etc.) reference
sections in `smallworld-architecture.md`.

---

## Phase A — Engine boot

| Step | Item | Status | Notes |
|---|---|---|---|
| A.1 | Instance, adapter, device | Exists | `GpuContext` in `gpu.rs` — windowed + headless paths, sRGB format, present mode |
| A.1 | `Capabilities` struct | Absent | `negotiate_features()` only checks `TIMESTAMP_QUERY`. No ray query, mesh shader, `SHADER_I16`, HDR color space gating. No feature-dependent pipeline variant selection |
| A.2 | Job pools | Exists | `RayonScheduler` in `jobs.rs` — `spawn`, `spawn_after` with atomic prereq counting, `parallel_for`, `join`. Auto-sizes workers (cores − 4, min 2). Named threads |
| A.2 | Job priority tiers | Absent | Single rayon pool, no background vs frame-critical distinction, no priority promotion |
| A.2 | Named threads (audio, render) | Absent | All work goes through the rayon pool |
| A.3 | Brick pool | Exists | `brick_pool.rs` — generation handles, free-list, pooled GPU alloc, occupancy masks |
| A.3 | Mesh pool (arena) | Absent | `MeshCache` in `stream.rs` uses per-mesh buffers, not a suballocating arena |
| A.3 | Staging ring buffer | Absent | All uploads via `queue.write_buffer()`. No ring, no segments, no fence-gated recycling |
| A.3 | AS pool | Absent | No acceleration structures |
| A.4 | Pipeline compilation | Exists | Synchronous in `Engine::new()`, `GBufferPass::new()`, `LightingPass::new()`, `Raymarcher::new()`. Shaders in `shaders.rs` with compile-time baking + runtime override via `$SMALLWORLD_SHADER_DIR` |
| A.4 | Async pipeline / hide behind menu | Absent | No async compilation, no menu to hide behind |
| A.5 | Input devices | Exists | `input.rs` — frame-captured snapshot, keyboard, mouse. Controller struct exists but not wired to winit gamepad |
| A.5 | Audio device | Absent | |
| A.5 | Script runtime | Absent | No Rhai/Lua |
| A.5 | UI theme | Absent | egui deps declared in `Cargo.toml` but never imported |

## Phase B — World mount / unmount

| Step | Item | Status | Notes |
|---|---|---|---|
| B.1 | Skeleton build (coarse SVO) | Absent | Exists in `sandbox_old/coarse_svo.rs` (32k lines) but not in current engine. Current `Svo` accepts brick insertions but has no worldgen-driven construction pass |
| B.2 | Residency prime | Exists | `BrickPager::preload_all()` and `preload_radius()` block until loaded |
| B.3 | Ready signal | Absent | No formal "presentable" signal |
| — | Mount as named lifecycle | Absent | No `WorldMount` concept — sandbox starts feeding bricks immediately |
| — | Unmount: cancel in-flight | Absent | `BrickPager` has no cancellation tokens |
| — | Unmount: wait copy fences | Absent | No fences (no staging ring) |
| — | Unmount: invalidate temporal | Absent | No temporal resources exist |
| — | Unmount: reset budget | Absent | No budget controller reset path |
| — | Empty world no-op safety | Incomplete | Cull is a no-op passthrough, stream handles empty sets, GBuffer handles zero meshes. `LightingPass` and blit not verified for zero-entry GBuffer. No designed-for empty state |
| — | Input priority routing | Absent | No menu vs gameplay distinction, no UI-first-refusal |
| — | Simulate tick gating | Absent | `App::update()` called every frame with wall-clock dt, no pause/resume concept |

## Stage 0 — Input and simulate

| Substage | Item | Status | Notes |
|---|---|---|---|
| 0a | Event pump | Exists | `ApplicationHandler::window_event()` in `engine.rs` accumulates winit events |
| 0a | UI first refusal | Absent | No UI system to capture |
| 0a | Input snapshot double-buffer | Incomplete | Snapshot stable during `App::update()` but no explicit double-buffer struct — `begin_frame()` clears edge-triggered sets. Architecture now names `FrameView` as the extraction boundary; renderer should never touch `World` directly |
| 0b | Fixed timestep accumulator | Absent | Single `App::update(dt)` per frame |
| 0b.1 | Script tick | Absent | |
| 0b.2 | Entity update | Incomplete | `World` has SlotMap storage with `ChangeSet` dirty tracking. No gameplay logic framework, no spawn/despawn scheduler |
| 0b.3 | Physics | Absent | No rigid body, no swept AABB, no GJK/EPA |
| 0b.4 | Field simulation | Absent | |
| 0b.5 | Water simulation | Absent | |
| 0b.6 | Weather | Absent | |
| 0b.7 | Edit queue | Absent | No double-buffered voxel edit collection. `ChangeSet` tracks object-level changes, not voxel edits. Architecture specifies swapped at `FrameView` extraction point |

## Stage 1 — Resolve

| Substage | Item | Status | Notes |
|---|---|---|---|
| 1.1 | `BrickSource` trait | Exists | `brick_source.rs` — trait for game-provided worldgen |
| 1.1 | `BrickPager` | Exists | `brick_pager.rs` — background workers, residency states (`Unknown → Loading → Resident → MipOnly`), LRU eviction, SSE-based demand |
| 1.1 | GPU worldgen | Absent | Exists in `sandbox_old/gpu_worldgen.rs` + WGSL shader. Not in current engine |
| 1.1 | CPU worldgen | Absent | Exists in `sandbox_old/worldgen.rs`. Current engine ships no built-in `BrickSource` |
| 1.2 | `MeshExtractor` trait | Incomplete | Trait exists in `stream.rs`. Only `PlaceholderExtractor` (box meshes) |
| 1.2 | Marching Cubes | Absent | |
| 1.2 | Dual Contouring | Absent | |
| 1.2 | Meshlet clustering / DAG | Absent | |
| 1.3 | Skeletal animation | Absent | No bone hierarchy, no blend trees, no IK |
| 1.3 | Entity pose evaluation | Absent | |
| 1.4 | Unified AABB buffer | Absent | `AABB` struct exists in `volume.rs`. `VoxelVolume::bounds()` and `Mesh::bounds()` compute them. No unified GPU buffer, no `AabbEntry` with `RenderableKind` |
| 1.4 | Skeleton state transitions | Incomplete | `BrickPager` has residency states. No unified skeleton spanning both bricks and meshlets |

## Stage 2 — Cull

| Substage | Item | Status | Notes |
|---|---|---|---|
| 2.1 | HZB build | Incomplete | `hzb.wgsl` shader exists (max-depth 2×2 downsample). `HzbBuilder` allocates mip-chain texture. **Compute dispatch never called** |
| 2.2 | Frustum cull | Absent | `CullStage::cull()` in `cull.rs` is a no-op passthrough returning all entries as visible. Interface accepts HZB for future use |
| 2.3 | Occlusion cull | Absent | Depends on HZB |
| 2.4 | SSE evaluation (GPU) | Absent | CPU-side version exists in `BrickPager::compute_demand()` |
| 2.5 | Meshlet cull | Absent | No meshlet system |
| 2.6 | Light cluster binning | Exists | `ClusteredLightGrid` in `lighting.rs` — CPU-side froxel, 80px tiles, 24 log-depth slices, max 32 lights/cluster. Uploads to GPU buffers |
| 2.7 | Shadow caster cull | Incomplete | `LightingPass::render()` filters by `casts_shadows` flag on CPU. No GPU frustum cull against shadow ortho frustum |
| 2.8 | Visibility readback | Absent | |

## Stage 3 — Stream

| Substage | Item | Status | Notes |
|---|---|---|---|
| 3.1 | Request ordering | Exists | `StreamStage::stream()` sorts by distance. `BrickPager::compute_demand()` sorts by SSE |
| 3.2 | Ring buffer staging | Absent | `queue.write_buffer()` only |
| 3.3 | Copy submit (fenced) | Absent | No transfer queue, no fencing |
| 3.4 | Budget + eviction | Exists | `MeshCache` 256 MB cap + distance eviction. `BrickPager` LRU eviction by frame-of-last-use. `StreamStage` 8 MB/frame upload cap, 128 max uploads/frame |
| 3.5 | Acceleration structures | Absent | `BvhNode` in `bvh.rs` is CPU-side BVH for raymarcher instancing, not hardware AS |

## Stage 4a — Geometry

| Pass | Item | Status | Notes |
|---|---|---|---|
| 4a.1 | Entity shadow depth | Exists | `ShadowAtlas` in `lighting.rs` — 4096² atlas, directional lights, depth-only, viewport/scissor per light region. `shadow.wgsl` |
| 4a.2 | Rasterizer GBuffer | Exists | `GBufferPass` in `gbuffer.rs` — 5 targets (depth, albedo, normal, material, emissive). Double-sided materials. Texture maps with fallback. `gbuffer.wgsl` |
| 4a.3 | Raymarcher GBuffer | Exists (not integrated) | `Raymarcher` in `raymarcher.rs` — full compute raymarcher with SVO traversal, BVH instancing, SSE LOD, own internal GBuffer. `raymarch.wgsl` (869 lines). **Not wired into `Engine::render_frame()`** — engine uses rasterizer path only |
| 4a.4 | Composite | Absent | No depth-aware merge. Two renderers exist independently |

## Stage 4b — Lighting

| Pass | Item | Status | Notes |
|---|---|---|---|
| 4b.1 | Shadow term (entity map) | Exists | Directional shadow map sampling with Godot-style receiver bias (normal offset + depth bias) in `shade.wgsl` |
| 4b.1 | Shadow term (SVO raymarch) | Exists (not integrated) | Raymarcher has its own shadow pass. Not connected to the deferred lighting pass |
| 4b.1 | Combined shadow `min(svo, entity)` | Absent | |
| 4b.2 | GI — cone tracing / SDFGI | Absent | |
| 4b.2 | GI — probes | Absent | |
| 4b.2 | GI — cone-traced AO | Absent | |
| 4b.3 | Clustered lighting | Exists | Full Cook-Torrance PBR in `shade.wgsl` — GGX, Smith geometry, Schlick fresnel. Froxel lookup. Directional + point + spot. HDR output (Rgba16Float) |
| 4b.4 | Volumetrics — height fog | Absent | |
| 4b.4 | Volumetrics — god rays | Absent | |
| 4b.4 | Volumetrics — clouds | Absent | |
| 4b.5 | Water rendering | Absent | |

## Stage 4c — Post-processing

| Pass | Item | Status | Notes |
|---|---|---|---|
| — | SSR | Absent | sw-49724f spec exists |
| — | SSGI | Absent | |
| — | TAA | Absent | sw-49724f spec exists. **Blocks SSR, SSGI, volumetrics, GI denoiser** |
| — | Bloom | Absent | sw-49724f spec exists |
| — | Motion blur | Absent | sw-49724f spec exists |
| — | Depth of field | Absent | sw-49724f spec exists |
| — | FSR upscale | Absent | |
| — | Tone mapping | Incomplete | Reinhard `hdr/(hdr+1)` in `blit.wgsl`. Architecture calls for ACES + optional AgX + HDR output |
| — | Color grading / LUT | Absent | |

## Stage 4d — UI and present

| Pass | Item | Status | Notes |
|---|---|---|---|
| 4d.1 | UI draw / egui | Absent | `egui`, `egui-wgpu`, `egui-winit` declared as workspace deps but never imported |
| 4d.2 | Present | Exists | `Engine::render_frame()` handles all `SurfaceTexture` error states (Lost, Outdated, Timeout, Occluded, Suboptimal). Reconfigures on Lost/Outdated |

## Cross-cutting structures

| Structure | Status | Notes |
|---|---|---|
| GBuffer: depth | Exists | Depth32Float |
| GBuffer: albedo | Exists | Rgba8UnormSrgb |
| GBuffer: normal | Exists | Rgba8Unorm (octahedral encoded) |
| GBuffer: roughness/metallic | Exists | Rgba8Unorm (r=rough, g=metal) |
| GBuffer: emissive | Exists | Rgba16Float |
| GBuffer: velocity | Absent | Required by TAA, motion blur |
| GBuffer: source flag | Absent | Required by composite, shadow ray offset |
| GBuffer: material/palette id | Absent | Required by debug, water detection |
| Temporal resources (owned set) | Absent | No double-buffered history, no teleport invalidation signal |
| `AabbEntry` / `RenderableKind` | Absent | Volumes and meshes iterated separately in cull stage |
| Edit queue (double-buffered) | Absent | |
| Serial pipes | Absent | |

## Entity architecture

| Item | Status | Notes |
|---|---|---|
| SlotMap + side structs | Exists | `World` uses `SlotMap<K,V>` for volumes, mesh instances, materials, lights. GPU singletons as side structs. Camera standalone |
| `ChangeSet` dirty tracking | Exists | Per-object spawn/despawn/mutate tracking |
| ECS deferred | Correct | Benchmarked (sw-cf6350), decision documented in CLAUDE.md |

## Build order (from architecture doc)

| Priority | Item | Status |
|---|---|---|
| 0 | `Capabilities` struct | Absent |
| 0 | Edit queue | Absent |
| 1 | GBuffer contract (velocity + source flag) | Absent — **blocks 4b and 4c** |
| 2 | Frame pipeline depth decision | **Decided** — depth 3: Sim(N+1) \| Render(N) \| GPU(N−1). Triple-buffered `FrameView` snapshot ring. Not yet implemented |
| 3 | Composite (both paths, matching formats) | Absent — raymarcher exists but not composited |
| 4 | Combined shadow term | Absent |
| 5 | TAA | Absent — **blocks SSR, SSGI, volumetrics, GI denoiser** |
| 6 | Clustered lighting | **Done** |
| 7 | GI (SDFGI over SVO) | Absent |
| 8 | Post chain | Only Reinhard tone mapping |
| ∥ | AS rebuild cost prototype | Not started |
