# Smallworld Engine Glossary


## Core Data

**Voxel** — The atomic world element. An 8-bit palette index (0 = air, 1–255 = solid
material). At base scale each voxel is 10 cm on a side (`VOXEL_SCALE = 0.1`). Instanced
objects can override this with a per-instance voxel scale.

**Brick** — A 16×16×16 cube of voxels (4 096 total) — the fundamental data unit. At base
scale one brick spans 1.6 m. No single `Brick` struct exists; data is split across three
GPU buffers managed by `BrickPool`: voxels (4 KB), palette (1 KB), mips (585 words).

**BrickData** — CPU-side transfer type carrying `voxels: [u8; 4096]` and
`palette: Vec<[u8; 4]>`. Used to move brick payloads between source, disk cache, and pool.

**BrickHandle** — Opaque CPU-side reference to a live brick in the pool. Carries a `slot`
(GPU buffer index) and a `generation` (use-after-free guard). `BrickHandle::NONE`
(`slot = u32::MAX`) marks an empty cell.

**Palette** — A per-brick 256-entry RGBA color table. Each voxel stores an index into this
palette, keeping voxel data at 1 byte while supporting diverse materials per brick.

**Occupancy** — The fraction of a mip cell that contains solid voxels. Stored in the alpha
channel of mip data (0 = fully air, 255 = fully solid). Drives LOD shading decisions and
planned ambient occlusion.


## GPU Storage

**BrickPool** — A pooled GPU allocator for bricks. Pre-allocates three storage buffers
(voxels, palettes, mips) at a fixed capacity. Manages slot allocation via a free-list with
generation-based validation. Both terrain (`BrickIndex`) and objects (`VoxelModel`)
reference slots in the same pool.

**BrickIndex** — A flat 3D grid mapping world-space brick coordinates to pool slot indices
(`u32`, or `u32::MAX` for empty). Backed by a GPU storage buffer with a CPU mirror. The
raymarcher does coarse DDA through this grid. The `BrickPager` populates it.

**CoarseMipGrid** — A persistent grid-parallel buffer storing mip levels 2–4 (73 words per
cell) for every loaded brick. Unlike the pool's mip buffer, this data survives eviction so
distant terrain renders at low resolution instead of vanishing.


## LOD & Mips

**Mip / Mip Chain** — Pre-computed downsampled representations of a brick's voxel data.
Four levels: 8³ → 4³ → 2³ → 1³ (total 585 `u32` words). Each word packs RGBA where
A = occupancy. Built by `compute_brick_mips()`.

**LOD (Level of Detail)** — Not mesh swapping — traversal termination. When SSE drops below
threshold the shader picks a mip level via `ceil(log2(threshold / sse))` and shades from
that instead of doing fine voxel DDA.

**SSE (Screen-Space Error)** — `voxel_scale × focal_length / distance`. The projected pixel
size of one voxel. When < 1–2 px the raymarcher stops fine DDA and shades from mip data.
Drives both rendering LOD and streaming demand (the pager demotes `Resident` cells to
`MipOnly` when SSE drops).

**Hot / Cold** — Residency concept. Hot bricks are actively being edited and must remain as
mutable pool bricks. Cold bricks are unchanged and eligible for SVDAG compression or pager
eviction.


## Streaming

**BrickSource** — Trait for providing brick data. Single method:
`generate(grid_pos, world_min) -> Option<BrickData>`. Must be `Send + Sync`, no GPU
access (runs on background threads).

**BrickPager** — Async streaming system. Each frame it drains completed loads, uploads to
GPU (capped at `max_uploads_per_frame`), walks the grid to classify cells by SSE, and
submits load/evict requests. Cell states:
`Unknown → Loading → Resident → MipOnly → (evictable)`, or `Air`.

**Region / RegionFile** — Disk persistence format for brick caching. Each region covers a
16³ cube of grid cells. Layout: 16 KB header (4 096 entries) + 4 KB sectors with zstd
compression.

**CachedSource** — A `BrickSource` wrapper that adds on-disk region file caching. Thread-safe
via `RwLock<HashMap>` over `Arc<Mutex<RegionFile>>`.

**GpuCachedSource** — Three-tier lookup chain: GPU cache (from `GpuWorldGenerator`) → disk
cache (region files) → CPU fallback (`WorldGenerator`).


## Rendering

**Raymarcher** — The GPU compute pipeline that renders the scene. Dispatches a fullscreen
compute pass (8×8 workgroups) that raymarches through terrain and BVH-traversed objects,
writing pixels to a storage texture. A second blit pass copies this to the window surface.

**DDA (Digital Differential Analyzer)** — The ray traversal algorithm. Two levels:
- *Coarse DDA* — steps through the `BrickIndex` grid (brick-sized steps, up to 512).
- *Fine DDA* — steps through individual voxels inside a hit brick (up to 64 steps).

**Blit Pass** — A fullscreen-triangle render pass that copies the compute raymarcher's
storage texture to the window surface. Vertices are generated from `vertex_index` (no
vertex buffer needed).

**Frustum** — The truncated pyramid defining the camera's visible volume. Bounded by the
near plane, far plane, and FOV. Rays are cast from the camera position through pixel
positions on the near plane. The engine does not do explicit frustum culling — the
raymarcher naturally skips geometry outside the frustum because rays simply don't hit it.

**Focal Length** — `screen_height / (2 × tan(fov_y / 2))`. Converts angular sizes to pixel
sizes for SSE computation.

**Shadows** — Sun shadow rays. Toggled via `FLAG_SHADOWS`. When enabled, each surface hit
casts a secondary ray toward the sun direction to test occlusion.

**Smooth Normals** — Occupancy-gradient normals. Toggled via `FLAG_SMOOTH_NORMALS`. Instead
of flat face normals from the DDA step axis, normals are derived from the gradient of
surrounding occupancy values, giving smoother shading on curved surfaces.


## GPU / wgpu

**GpuContext** — The wgpu device/adapter/queue wrapper. Negotiates a high-performance
adapter, enables timestamp queries if available, and creates the logical device.

**Binding / Bind Group** — wgpu/WebGPU mechanism for connecting GPU buffers and textures to
shader code. Each `@binding(N)` in WGSL corresponds to a buffer or texture. The compute
bind group has 9 bindings: uniforms, brick index, voxel data, palettes, output texture,
instances, packed grids, BVH nodes, mips.

**Workgroup** — A group of GPU threads that execute together. The raymarcher uses 8×8
workgroups (64 threads), each thread handling one pixel.

**Staging / Readback** — GPU → CPU data transfer pattern. A staging buffer with
`MAP_READ` usage receives data copied from a GPU-only buffer, then is mapped to CPU
memory. Used by `GpuTimestamps` for timestamp readback and `GpuWorldGenerator` for reading
back generated bricks.

**GpuTimestamps** — Per-pass GPU timestamps with EMA-smoothed readback. Records how long
each pass (compute, blit, egui) takes on the GPU.


## Scene & Objects

**VoxelModel** — Shared voxel data for an object type (tree, rock, prop). Contains a local
brick grid (slot indices into the global `BrickPool`), grid dimensions, and a per-instance
voxel scale.

**VoxelInstance** — Places a `VoxelModel` in the world with a position and rotation.
Produces a `VoxelInstanceGpu` for shader consumption.

**Scene** — Holds all instanced voxel objects. Concatenates model grids into a single packed
GPU buffer and builds a BVH over instance AABBs.

**BVH (Bounding Volume Hierarchy)** — A flat-array binary tree over instance AABBs for GPU
ray traversal. Each node stores an AABB and is either internal (two children) or leaf (up
to 4 instances). Built with centroid midpoint splitting.

**TLAS / BLAS** — Two-level scene model borrowed from RT terminology:
- *TLAS (Top-Level Acceleration Structure)* — the BVH over instance AABBs.
- *BLAS (Bottom-Level Acceleration Structure)* — each instance's local brick grid.
Rays traverse the TLAS first, then transform into object space for brick-grid DDA.

**AABB (Axis-Aligned Bounding Box)** — A (min, max) corner pair enclosing an instance in
world space. Used for BVH construction and ray-box intersection.


## Camera

**FreeCamera** — A fly-camera with position, yaw, pitch, FOV, aspect ratio, near/far
planes. Provides view and projection matrices.

**View Matrix** — Transforms world space → camera space (camera at origin, looking down −Z).

**Projection Matrix** — Transforms camera space → clip space. Uses a right-handed
perspective projection with DirectX depth conventions (0..1).

**Inverse View-Projection** — `(projection × view)⁻¹`. Used by the raymarcher to
reconstruct world-space ray directions from pixel coordinates.

**Camera Path** — A per-preset parametric path `(t) → (position, yaw, pitch)` used by the
benchmark harness to fly the camera through the scene deterministically.


## Worldgen

**Density Function** — The terrain is defined by a 3D scalar field, not a heightmap. A
point is solid when `density > 0`. Base density increases with depth:
`(terrain_base − y) / terrain_amp + noise`.

**FBM (Fractal Brownian Motion)** — Layered noise. Multiple octaves of value noise are
summed with increasing frequency and decreasing amplitude. Used for terrain shape and cave
carving.

**Strata** — Material layering based on density depth. Near-surface → grass/dirt,
mid-depth → stone, deep → darker stone. Gives terrain a natural layered appearance.

**Cave Carving** — Dual-noise intersection: two independent FBM fields are evaluated and
where both exceed `cave_threshold` the voxel is carved hollow. Produces winding, organic
cave networks.

**Water Table** — A global `water_level` height. Air voxels at or below this level are
filled with water (palette index 5).


## Shader System

**Compose** — Since WGSL has no `#include`, shared declarations (`common.wgsl`) are
concatenated on the Rust side via `shaders::compose()`. Baked into the binary with
`include_str!`; runtime override via `$SMALLWORLD_SHADER_DIR`.


## Planned / Referenced

**SVO (Sparse Voxel Octree)** — A hierarchical octree for voxel storage. Not currently
used. Referenced in design docs as a potential intermediate format.

**SVDAG (Sparse Voxel DAG)** — Extends SVO with shared subtree deduplication for 70–90%
compression. Planned (D4) as a background compression cache for cold/immutable data.

**Anchor** — (Not yet implemented.) Object base bricks contacting solid terrain. Terrain
edits dirtying anchor regions would queue re-anchor checks, enabling emergent undermining
mechanics.
