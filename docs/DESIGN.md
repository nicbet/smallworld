# smallworld — Technical Design

A voxel engine built in Rust + WGPU. The architecture is organized around an
**out-of-core (OOC) rendering pipeline** that runs every frame. Disk is the
source of truth; RAM and VRAM are caches. The engine streams, culls, and renders
only what the camera can see.

## Per-Frame Pipeline

Every frame executes four stages in order. Each stage has well-defined inputs and
outputs, and each is served by one or more engine subsystems.

```
Resolve ──▸ Cull ──▸ Stream ──▸ Execute
  │                                │
  └──── depth buffer feedback ◂────┘
```

**Stage 1 — Resolve.** Populate the world skeleton with volume data. Thread pool
workers generate voxels from seed (GPU or CPU worldgen) or load from disk cache.
Fill `VoxelVolume` implementations and register AABBs into the shared buffer.

**Stage 2 — Cull.** GPU compute passes determine what's visible. Frustum culling
tests AABBs against camera planes. The Hierarchical Z-Buffer (HzB) — a mip-chain
depth pyramid built from the previous frame's depth output — drives occlusion
culling. SSE evaluation classifies each surviving cell's LOD requirement. Output:
a visibility set and prioritized stream/eviction lists.

**Stage 3 — Stream.** Async transfer of resolved data to GPU memory. A ring
buffer allocator stages CPU-side data; copy queues (Metal blit encoder or Vulkan
async transfer queue) move it to VRAM. Fences gate segment recycling. A budget
controller enforces hard VRAM/RAM limits with LRU eviction.

**Stage 4 — Execute.** Render what's resident. Two execution paths — raytracing
and rasterization — can run independently or compose via a compositor for hybrid
output (e.g., rasterized near terrain + raytraced GI/shadows). Depth buffer
output feeds back to Stage 2 for next frame's HzB, closing the pipeline loop.

## Core Abstractions

### VoxelVolume

Trait representing any spatial voxel data structure. Exposes enough structure for
shader-level traversal optimization — not fully opaque.

```
volume_type() → VolumeKind    // enum: SVO, FlatGrid, ChunkedDDA
traversal_bindings()          // type-specific GPU bind groups
bounds() → Aabb               // world-space bounding box
lod_hint() → LodMeta          // LOD metadata for SSE decisions
```

Shaders dispatch optimized traversal kernels per `VolumeKind`. The renderer never
needs to know which volume type it's drawing — the trait absorbs the difference.

**Implementations:**

- **SvoVolume** — Sparse Voxel Octree backed by a `BrickPool`. Cursor-based
  front-to-back traversal with occupancy masks. Interior nodes store averaged
  color for coarse LOD. Best for large-scale terrain and distance rendering.

- **ChunkedVolume** — 32³ brick chunks in a spatial grid with BVH macro-level
  skip and DDA micro-traversal within each brick. The workhorse for near-field
  destructible terrain.

- **FlatGrid** — Uniform 3D array with simple DDA traversal. For small instanced
  props (trees, rocks, pebbles) and as a benchmark baseline.

### AABB Buffer

Dedicated GPU storage buffer for per-chunk and per-meshlet bounding boxes,
separate from volume data. Packed layout optimized for culling compute passes.
Any `VoxelVolume` registers its AABBs here.

### Depth Buffer

High-performance depth representation decoupled from render targets. The
execution stage writes it; the culling stage reads the previous frame's version
for HzB pyramid construction. This separation lets culling run ahead of shading.

### Renderer

Trait abstracting over execution paths:

```
prepare()          // per-frame setup
encode()           // record GPU commands
resize()           // handle window resize
output_texture()   // final color output
```

Two implementations: `Raymarcher` (compute shader, existing) and `Rasterizer`
(vertex/fragment pipeline with GPU-driven indirect draws from meshlet SSE
output).

### MeshExtractor

Trait abstracting over mesh extraction algorithms:

```
extract(volume, lod) → MeshData    // vertex + index + normal buffers
```

Implementations: Marching Cubes (baseline isosurface), Dual Contouring (sharp
edge preservation from Hermite data stored in extended brick format).

### BvhAccel\<T: Bounded\>

Generic bounding volume hierarchy. Reusable by `ChunkedVolume` macro structure,
the instance system, and meshlet culling.

## World Skeleton

Lightweight spatial hierarchy storing only AABBs, residency state, and LOD
metadata. Fits in RAM for arbitrarily large worlds. This is the OOC pipeline's
source of truth for what exists. Per-cell states:

```
Unknown → Loading → Resident → MipOnly → (evictable)
                                          Air (pruned)
```

Built at startup via coarse SVO construction: multi-threaded height sampling,
min/max mip pyramid, classification into air / buried-solid / surface-crossing.

## Culling Pipeline

All culling runs as GPU compute, reading the AABB buffer.

1. **Frustum culling** — 6-plane AABB test, outputs visibility bitfield.
2. **HzB construction** — mip-chain depth pyramid from previous frame's depth.
   Power-of-two downsample, each level stores max depth of 2×2 parent region.
3. **Occlusion culling** — project surviving AABBs to screen-space, sample
   coarsest HzB mip that covers the projection. Conservative: never culls
   visible geometry.
4. **SSE evaluation** — `voxel_scale × focal_length / distance` per cell.
   Unified visibility + LOD decision in one dispatch. Replaces CPU-side
   `BrickPager::classify_demand`.
5. **Visibility readback** — GPU → CPU. Diff visible set against resident set
   to emit stream requests and eviction candidates.

## Streaming Architecture

### Thread Pool

Split into dedicated pools sized independently:
- **I/O pool** (2–3 threads): disk reads + zstd decompression
- **Compute pool** (4–6 threads): worldgen + meshing

### Ring Buffer

Fixed-size cyclic upload heap replacing per-brick `write_buffer` calls. Workers
lock a segment, write data, release. Wraparound when the end is reached.

### Copy Queues

**Metal path:** Cyclic `MTLBuffer` sized for N frames in flight (typically 3).
`MTLBlitCommandEncoder` for `copyFromBuffer` staging → VRAM transfers.
`dispatch_semaphore` / frame-in-flight counter prevents CPU overwriting segments
the GPU is still reading.

**Vulkan path:** Dedicated async transfer queue where available. Fence insertion
after copy submission; poll on subsequent frame; recycle staging segments only
after GPU confirms transfer complete.

### Budget Controller

Hard VRAM and RAM limits enforced before allocation. Eviction policy: LRU
weighted by SSE and visibility age. Panic eviction path for sudden camera
teleports requiring fast memory reclamation.

### Priority Scheduling

Load requests ordered by (visible × SSE). Cancellation tokens for loads that
become irrelevant when the camera moves. Integrates with culling stage visibility
output.

## Data Sources

Voxel data enters the pipeline through the `BrickSource` trait:

- **GPU worldgen** — compute shader noise (FBM terrain + cave carving). Primary
  path; generates bricks in batches of 256. Fastest source, even outperforms
  disk reads by ~3×.
- **CPU worldgen** — hash-based value noise fallback. Bit-identical to GPU
  version modulo FMA contraction differences.
- **Disk region cache** — 16³ brick regions, zstd-compressed. Persists generated
  data across runs.

Pipeline: GPU worldgen (fast) → disk cache (persists) → CPU fallback (gap fill).

## Brick Data

The fundamental voxel unit: 16³ = 4096 voxels per brick.

- **Voxels:** 8-bit palette index per voxel (0 = air), packed 4 per u32
- **Palette:** 256-entry RGBA, 1 KB per brick
- **Occupancy mask:** 64 bits, one per 4×4×4 sub-chunk, enables fine DDA skip
- **Hermite data** (extended format): per-edge intersection point + surface
  normal for Dual Contouring. Optional; bricks without Hermite data use standard
  format.

## Meshing Pipeline

Mesh extraction converts volume data to vertex/index buffers for rasterization.

### Extractors

- **Marching Cubes** — isosurface from density field. Standard 256-entry lookup.
  Vertex normals from gradient estimation. Baseline quality.
- **Dual Contouring** — QEF minimization for vertex placement. Reads Hermite
  data from extended brick format. Preserves sharp edges on architectural and
  blocky geometry.

### Meshlets

Extracted meshes are decomposed into 64–128 triangle clusters with a DAG
hierarchy for LOD. Evaluate `meshoptimizer` crate for clustering +
simplification; roll own Nanite-style pipeline if integration creates too much
churn. Per-meshlet bounding sphere + normal cone for culling.

### GPU Meshlet SSE

Compute pass evaluating screen-space error per meshlet against camera. Outputs
indirect draw buffer selecting parent (coarse) or children (fine) meshlets.
Feeds directly into the rasterizer's `draw_indexed_indirect` calls.

### LOD Transitions

- **Geomorphing** — vertex interpolation between LOD levels over a transition
  distance. Smooth, no popping.
- **Dithered blending** — screen-door dissolve (Bayer matrix) fading between
  LODs. Cheaper than geomorphing.

Selectable per-material.

### Chunk Stitching

Transvoxel transition cells or geometry skirts to seal cracks between adjacent
chunks at different LOD levels.

## Hybrid Rendering

The compositor merges raytracer and rasterizer G-buffer outputs. Both write
matching formats (position, albedo, normal). Depth-aware blending: closer surface
wins per pixel. Shared lighting model (ambient + directional with shadow term).

Three runtime modes:
- **Raytrace-only** — current behavior, compute shader path
- **Rasterize-only** — vertex/fragment path with meshlet indirect draws
- **Hybrid** — rasterize near terrain + raytrace distant terrain, GI, shadows

Switchable at runtime for profiling and debugging.

## ECS

Entity Component System adopted after A/B benchmark of `bevy_ecs` vs `hecs` at
multi-million entity scale. Provides the compositional model for the hybrid
engine: a chunk can independently have volume, mesh, visibility, and residency
components.

## Job System

Persistent worker pool initialized at engine start. No per-task thread spawning.

### Worker Pool

Priority queues with work-stealing across threads. Job categories map to queue
priority: frame-critical (meshing a visible chunk) runs before background
(pre-caching a distant region). Workers pull from their own queue first, then
steal from others when idle.

### Dependency Graph

Jobs declare dependencies: "mesh chunk X after worldgen X completes." The
scheduler topologically sorts and dispatches. Main-thread result callbacks
deliver finished work (uploaded meshes, generated bricks) without polling.

The streaming pipeline's I/O and compute pools (see Streaming Architecture) are
specialized partitions of this system — the job system is the general-purpose
foundation they sit on.

## Audio Engine

Multi-channel mixer with spatial positioning. The engine provides the mixing,
spatialization, and effects infrastructure; games supply the actual sound assets.

### Mixer

Per-channel play/stop/volume/pan. Channels are lightweight handles — hundreds
can exist simultaneously. Ducking and cross-fading between channels for smooth
transitions (e.g., combat music fading over exploration ambience).

### Spatial Audio

3D-positioned sources with distance attenuation. Listener position and
orientation derived from the camera entity. Attenuation model: inverse-distance
with configurable rolloff. Stereo panning from source-listener angle.

### Effects Bus

Post-mix processing chain: reverb, low-pass filter, echo. Effects are composable
— a cave environment applies reverb + low-pass; outdoors applies none. Per-source
effect send levels allow mixing dry and wet signals.

## UI Framework

Engine-level layout and widget system. Games define screens, menus, and HUD
elements using engine primitives; the engine handles layout, input routing, and
rendering.

### Layout Engine

Panel-based hierarchy with flex layout, anchoring, and scrolling. Panels can be
positioned absolutely (HUD overlays) or flow within a flex container (menus,
inventories). Scroll containers clip and virtualize long content.

### Primitives

Button, checkbox, dropdown, slider, text input. Each primitive emits interaction
events (clicked, changed, submitted) through the ECS event system. Primitives
are composable — a settings screen is a panel of sliders and dropdowns.

### Theming

Style objects defining colors, fonts, spacing, and border radii. A theme is a
named collection of styles applied globally. Games provide their own themes; the
engine ships a debug default. Hot-reloadable for iteration.

## Scripting Runtime

Embedded scripting language for game logic. The engine provides sandboxed
execution and API bindings; games provide the scripts.

### Runtime

Rhai or Lua (evaluated during implementation). Sandboxed: scripts cannot access
the filesystem, network, or unsafe memory. Execution is budgeted per frame to
prevent runaway scripts from stalling the render loop.

### Engine API Bindings

Scripts can:
- Query and modify voxel data (read/write materials, place/remove blocks)
- Spawn and control entities (position, velocity, components)
- Play audio (trigger sounds, set spatial position)
- Drive UI (show/hide panels, update text, respond to input events)
- Read input state (keyboard, mouse, gamepad)
- Access field simulation channels (temperature, moisture, custom)

Bindings are registered at engine init. Games extend with game-specific bindings
(crafting recipes, quest state, etc.).

## Field Simulation

Generic scalar field propagation on the voxel grid. The engine provides storage
and solver; games define what the fields represent (temperature, moisture, magic,
etc.).

### Storage

Sparse per-brick float channels. A field is identified by name. Only bricks with
non-default values allocate storage — air and homogeneous regions cost nothing.

### Diffusion Solver

Per-tick propagation on active bricks. Each tick, values diffuse to neighbors
weighted by a per-field diffusion coefficient. Active set is the union of bricks
with non-default values and their face-adjacent neighbors. Convergence: inactive
bricks (no change above epsilon) are removed from the active set.

### Query API

Read/write field values at arbitrary world positions. Script bindings expose
fields to game logic. Rendering can sample fields for visual effects (heat haze,
frost, bioluminescence).

## Atmosphere & Weather

Environmental rendering and simulation. The engine provides fog, volumetric
effects, and weather state; games configure biome-specific parameters.

### Global Weather State

Uniform buffer updated per frame: wind vector, fog density, precipitation
intensity, time-of-day, cloud coverage. Driven by game logic or scripted weather
sequences.

### Fog

Height/depth fog accumulated along the primary ray march. Density varies with
altitude (denser in valleys) and weather state. Integrated into the shade pass —
no separate fog pass needed.

### God Rays

Sun-visibility sampling along the primary march. Accumulates in-scatter when
march steps are lit by the sun (not in shadow). Modulated by fog density for
volumetric appearance.

### Clouds

Volumetric cloud layer above the terrain ceiling. Noise-driven density field
sampled by the sky model. Lit by sun with self-shadowing. Cloud coverage and
altitude controlled by weather state.

### Precipitation

Particle-based rain and snow. Surface wetness: a per-brick scalar field
(see Field Simulation) tracking moisture accumulation. Puddle formation in
concavities. Visual: darkened albedo + specular gloss on wet surfaces.

## Water

Fluid representation, rendering, and simulation. Voxel-native — water is a
material in the brick grid, not a separate mesh.

### Representation

Water is a voxel material with an active-simulation flag. Bricks containing
water are added to the simulation active set. Surface detection: water voxels
adjacent to air are surface voxels and receive special shading.

### Rendering

Transmission segment in the raymarcher: when a ray enters water, switch to
refraction (Snell's law at the surface normal) and Beer-Lambert depth tint
(color shifts toward deep blue/green with distance). Surface shading: animated
normals (scrolling noise), screen-space reflections, cone-traced reflections for
rough water.

### Flow Simulation

Cellular automaton on the active brick set. Per-tick: water flows downward
(gravity), laterally (pressure equalization), and accumulates in basins.
Waterfalls: vertical flow into air creates splash particles. Flow rate modulated
by the global wind vector from weather state.

## Technology Stack

- **Rust** (edition 2024, resolver 3) — memory safety, data-race freedom,
  concurrency
- **WGPU** — cross-platform GPU abstraction (Metal, Vulkan, DX12)
- **glam** — linear algebra
- **crossbeam** — multi-producer/consumer channels
- **bytemuck** — zero-copy GPU data casting
- **zstd** — region file compression
- **egui** — debug UI overlay

## Crate Structure

```
crates/
  engine/     smallworld-engine (library)
              OOC pipeline, volumes, rendering, streaming, culling,
              job system, audio, UI, scripting, field simulation,
              atmosphere, water
  sandbox/    smallworld-sandbox (binary: smallworld)
              Worldgen, scenes, camera, debug UI, GPU worldgen
```

Games depend on `smallworld-engine` and provide content: assets, biomes,
creatures, scripts, UI themes, sound banks. The engine provides mechanisms;
games provide policy and content.
