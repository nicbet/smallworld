# smallworld

A micro-voxel engine in Rust + wgpu. Compute-shader raymarching, no meshes.

## Architecture

The engine represents the world as 16x16x16 **bricks** of 8-bit palette-indexed voxels. Rendering is a fullscreen compute pass that raymarches through two data structures:

- **Dense grid** (`BrickIndex`) — a flat 3D grid of brick handles for large continuous volumes (terrain, buildings, caves). Traversed via coarse DDA.
- **Instanced volumes** (`Scene` + `VoxelModel` + `VoxelInstance`) — small voxel models with per-instance transforms, traversed via BVH. Each model has its own brick grid and voxel scale.

What you put in each is your game's concern. The engine doesn't distinguish terrain from buildings from dungeon floors.

## Workspace

```
crates/
  engine/     smallworld-engine (library)
  sandbox/    smallworld-sandbox (dev/test binary)
```

The engine is a pure library with no windowing, input, or UI. The sandbox is a developer tool that exercises engine features with an egui debug overlay.

## Quick start

```bash
make sandbox      # run the sandbox (default scene)
make bench        # 20s benchmark with metrics report
make smoke        # headless adapter probe (CI)
make test         # workspace tests
make lint         # fmt --check + clippy
```

## Engine modules

| Module | Purpose |
|---|---|
| `gpu` | wgpu device/adapter negotiation, surface configuration |
| `brick_pool` | Pooled GPU allocator for 16³ bricks (voxels + palettes + mips) |
| `brick_index` | Flat 3D grid mapping world coordinates to brick handles |
| `mip` | Intra-brick mip chain (8³→4³→2³→1³) for LOD |
| `voxel_object` | `VoxelModel` (shared data) + `VoxelInstance` (transform) |
| `scene` | Instance collection with BVH build and GPU upload |
| `bvh` | Flat-array BVH over instance AABBs |
| `raymarcher` | Compute + blit pipelines, uniform management, bind groups |
| `camera` | `FreeCamera` with WASD + mouse look |
| `gpu_timing` | Timestamp queries with EMA smoothing |
| `shaders` | Baked WGSL sources with runtime override support |
| `assets` | Path resolution (dev root, exe-adjacent, env override) |

## Using the engine

```rust
use smallworld_engine::*;

// 1. Create GPU context
let instance = gpu::GpuContext::create_instance();
let surface = instance.create_surface(window.clone())?;
let gpu = gpu::GpuContext::new(instance, &surface).await;

// 2. Allocate brick storage
let mut pool = brick_pool::BrickPool::new(&gpu.device, 32768);
let mut index = brick_index::BrickIndex::new(
    &gpu.device,
    [32, 12, 32],                           // grid dimensions in bricks
    glam::Vec3::new(-25.6, -9.6, -25.6),    // world-space minimum corner
);

// 3. Fill bricks
let handle = pool.alloc().expect("pool exhausted");
pool.write_voxels(&gpu.queue, handle, &voxel_data);
pool.write_palette(&gpu.queue, handle, &palette);
let mips = mip::compute_brick_mips(&voxel_data, &palette);
pool.write_mips(&gpu.queue, handle, &mips);
index.set([x, y, z], handle);
index.upload(&gpu.queue);

// 4. Add instanced objects
let mut scene = scene::Scene::new();
let model_id = scene.add_model(model);
scene.add_instance(voxel_object::VoxelInstance {
    model_id,
    position: glam::Vec3::new(0.0, 2.0, 0.0),
    rotation: glam::Quat::IDENTITY,
});
scene.upload(&gpu.device);

// 5. Create raymarcher and render
let raymarcher = raymarcher::Raymarcher::new(
    &gpu, width, height, surface_format, &pool, &index, &scene,
);
raymarcher.render(
    &gpu, &mut encoder, &view, &camera, &index, &scene,
    flags, sse_threshold, compute_ts, blit_ts,
);
```

## Voxel data format

Each brick is 16x16x16 = 4096 voxels. Each voxel is an 8-bit index into a per-brick 256-entry RGBA palette. Index 0 = air (transparent).

Voxels are packed 4-per-u32 in the GPU buffer. The palette is stored as packed RGBA u32s. Mip data is 585 u32 words per brick (4 levels of pre-averaged RGBA).

## Rendering pipeline

1. **Compute pass** — fullscreen raymarcher writes to a storage texture. Two-level DDA: coarse through the brick grid, fine inside individual bricks. SSE-driven LOD skips fine DDA for distant bricks and shades from mip data.
2. **Blit pass** — copies the storage texture to the surface via a fullscreen triangle.
3. **UI pass** — egui overlay (sandbox only, not part of the engine).

## Sandbox features

- Scene presets: Default, Terrain Only, Objects Only, Stress, Single Brick, Empty
- Runtime preset switching via egui dropdown
- Debug panel: adapter info, resolution, render scale, shadows, smooth normals, SSE threshold
- Frame time graph with CPU/GPU/dt breakdown
- Benchmark mode: `--bench [preset] [--duration N]` with deterministic camera paths and JSON output

## Requirements

- Rust 1.97+ (edition 2024)
- wgpu 30 (Metal on macOS, Vulkan on Linux/Windows)
- A GPU with compute shader support
