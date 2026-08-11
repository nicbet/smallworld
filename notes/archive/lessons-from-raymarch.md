## Introduction

okay so yesterday we spent all day implementing and performance optimizing ray marching. I think it's time we shift gears a little bit. Getting and image on screen for a voxel world is really a collection of separate concerns that need to be performant together.

## 1. World Generation

We learned that generating on the CPU works for smaller worlds, but is MUCH fsater on GPU. So fast that it even outperforms loading cached regions from disk by 3x. A "Hybrid Generator" seems to be the correct path formward. World noise on GPU. Objects like pebbles, trees, etc. as a few variations from sophisticated (i.e. not a trunk and ellipsiod on top) "recipes" on CPU, rendered once to memory, pointer to a random variant into the world.

## 2. Data Structures

We learned that memory efficient data structures that GPUs love cache coherency and predictable memory lookups. They hate chasing pointers.

- Worlds of _any_ useful size beyond a few meters must be streamed
- Successful approach appears to be parallel worker threads that run worldgen from seed, cache to disk
- Depending on image generation different data structures are efficient

### DDA (Digital Differential Analyzer)

Highly optimized raymarching algorithm, used to traverse a uniform 3d grid.
Philosophy: memory is cheap, math needs to be predictable
Performance: extremely fsat per-ray-step execution, grid uniform, memory lookups predictable, great cache performance.
Problem: DDA cannot skip empty empty space, must evaluate all empty "air" voxels. Storing large worlds in uniform 3d array VERY expensive.

### SVO (Sparse Voxel Octrees)

Hierarchical structure, world represented as massive bounding cubem subdivides into 8 smaller cubes (Octants) recursively down to max resolution.
Philosophy: memory bandwidth is bottleneck, compress data, skip empty space
Performance: incredibly memory efficient, can skip empty sky, reduce traversal time from O(n) to (log n),
Problem: Ray traversal requires "pointer chasing" which GPUs hate and ruins cache coherence.

### Chunked DDA

Modern compromise, when maintaining pure SVO is too slow. World is divided into 32x32x32 voxel volumes (bricks). Within chunk data is uniform grid,
rays are cast using DDA. Macro world manages these chunks using a spacial acceleration structure like Octree or BVH to quickly skip over empty chunks
before DDA algorithm chases rays.

## 3. Rendering approaches

There are fundamentally two different rending approaches:

### A. Raytracing

GPU shoots ray for every pixel. Complexity grows with pixels, geometry never explicitly generated.
Performance bound is fragment-shader and fill-rate limitation
GPU Architecture is hostile (thread divergence), pointer chasing through a voxel tree ruins performance
Memory (VRAM) is high: must store whole volume
Destructibility: O(1) only store memory edit
Animations: Extremely difficult

### B. Polygonal Meshing:

Precalculate 2d surface shell from 3d volume, convert to vertices and triangles that get rasterized like traditional 3d game
Performance bound is vertex shader and polygon count limitation
GPU Architecture is native (GPU built for rasterization)
Memory (VRAM) is low (only store surface shell)
Destructibility: Delayed (requires remeshing chunk)
Animations: Trivial (standard bone matrices)

## 4. Level of Detail

- Raytracing handles infinite scaling and LOD inherently. As a ray travels further from camera, its footprint grows, testing against higher resolution parents in octree. Massive view distances and no LOD popping.
- Meshing struggles with view distance, must generate further away meshes as lower resolutions, stiching low-res to high-res chunks creates seams and cracks in the world, but can be repaired with transition algorithms like Transvoxel, Geomorphing, Dual Contouring, or Dithered LOD blending.
- SSE is the gold standard. However classic (CPU driven) approach is terribly inefficient: evaluates massive chunks at once, mountain range with one peak causes entire mountain range to render at LOD0 to satisfy threshold for peak. Modern approach move evaluation from CPU to GPU. Modern pipelines process models into hierarchy of tiny clusters called Meshlets (64-128 triangles each), organized in a DAG tree structure, GPU evaluates SSE for each meshlet individually and in parallel, if drops below SSE error threshold, render simplified parent, too high SSE split and render higher-detail children.
- Use Frustum Culling when we have hierarachical data like SVO or macro-chunk grids. Test bounding boxes of macro chunks against camera, fast discard. Just like SSE, traditional was CPU, modern is GPU based (metal, Vulkan native)
- Use Occlusion Culling (Hierarchical Z-Buffer or HzB) for every voxel inside frustum cone: compute shader compares bounding boxes of chunks against low resolution depth map, cull before rasterization.
- Out of Core rendering essential for voxel/microvoxel engines: run mathematical checks (FC, HzB) to get out-of-core rendering. Don't stream high-res data and send to raytracer or mesher that we cannot even see. Because voxel volume data is astronomically on memory (RAM, VRAM), hard drive is ultimate source of truth for what needs to be loaded in current frame. Maintain low-memory skeleton of world: just bounding boxes of SVO nodes or chunks. Every frame (or every few) run frustum culling (chunk discarded if not in camera) -> occlusion culling (using low res depth buffer) -> SSE -> Streaming+Allocation (async request to stream exact voxel data to memory) -> Execution (Raytrace+Mesh)

## 5. Streaming Architecture

Stutter free chunk streaming requires compeltely decoupling disk I/O, CPU meshing, GPU upload into async stages. Ensure main thread and render queue never wait for data.
Modern pipeline is:

### 1. Thread Pool (CPU)

Don't launch new thread for every chunk, causes massive context switching overhead. Initialize persistent thread pool at engine start, 4-6 threads on modern CPU like 10700K exclusively for chunk processing, 2 threads strictly for async file reads of cached compressed voxel data from SSD drive. Remaining threads execute meshing algorithm to generate vertex and index arrays in RAM.

### 2. Staging Buffer (CPU to GPU)

Allocate Ring Buffer in Upload Heap. When worker finishes meshing a chunk locks segment of ring buffer and writes vertex data to it. Thread never stalls waiting on GPU.

### 3. Dedicated GPU Copy Queues

Instead of memory transfers for primary graphics queue use dedicated copy queue, runs async in background, pull vetex data from staging buffer into GPU VRAM.

### 4. Fence Synchronization

When submitting copy queue, insert GPU fence marker to signal transfer complete, staging buffer segment then marked "free" to be overwritting by next incoming chunk.

## 6. Animation

- Raytracing is fundamentally rigid, applying animation is mathematically brutal
- Polygonal meshin makes animations trivial

## 7. Industry Standards

Because of extreme trade-offs, almost no engine relies on one technology or the other. Most modern voxel engines rely on hybrid approach:

- Polygonal meshing for players objects and close-up terrain
- Ray tracing against SVO strictly for rending ultra distant terrain, shadows and global illumination.

## 8. Engine Technologies

- C++: unmatched ecosystem, PhysX, Havoc, Tracy, RenderDoc, grpahics SDKs. max performance. Cons: memory mgmt, treads safety pitfalls, cross-platform nightmare.
- Rust: guaranteed memory safety, data-race free, great for concurrent engines, cross platform story. Cons: small ecosystem, strict ownership fights OO patterns, forcing desvs into ECS architectures, shaders stalled, better WGSL or GLSL.
- Vulkan: highly explicit, low level designed to minimize CPU overhead, notoriously verbose, hardware vendor specific quirks.
- Metal: ergonomic, dev friednly, design for apple unified memory, only runs on macOS, iOS. Can be translated to Vulkan via MoltenVK.
- WGPU: web gpu for Rust. Write code and shaders once, wgpu translates into native metal, vulkan or directX. goldilocks performance balance.
- Dawn: basically WGPU for C++
