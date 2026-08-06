
# GPU Compute Noise Generation — Implementation Walkthrough

## What was built

A compute shader that generates terrain voxel data on the GPU, replacing CPU noise evaluation as the primary generation path. The engine is unchanged — this is entirely sandbox code.

## Architecture

### Compute shader: `crates/sandbox/shaders/worldgen.wgsl`

A direct port of the CPU noise pipeline from `worldgen.rs`:
- `wg_hash(x, y, z, seed)` — same integer hash (WGSL u32 wraps naturally)
- `hash_f()` — hash to [0,1] float
- `smoothstep3()` — Hermite interpolation (renamed from `smooth` which is a WGSL reserved word)
- `noise3d()` — trilinear value noise from 8 corner hashes
- `fbm3d()` — fractal Brownian motion with configurable octaves
- `sample_material()` — density + cave + strata logic, identical to CPU

Entry point: `@compute @workgroup_size(256, 1, 1)` — one workgroup per brick, 256 threads, each thread handles 16 voxels (4 words of 4 packed voxels each). No cross-thread contention because each thread writes consecutive complete u32 words.

Bindings:
- `@binding(0)` uniform: `GenParams` (seed, terrain params, world_min, brick_size)
- `@binding(1)` storage read: `requests` — `vec4<u32>` grid positions per brick
- `@binding(2)` storage read/write: `output` — packed voxel data, 1024 u32/brick

### `gpu_worldgen.rs` — dispatch and readback

`GpuWorldGenerator` manages the compute pipeline and a shared results cache.

**`generate_all(dims, device, queue)`** processes all grid cells in batches of 256:
1. Write grid positions to the request buffer
2. Dispatch compute (256 workgroups per batch)
3. Copy output buffer → staging buffer (MAP_READ)
4. `device.poll(PollType::wait_indefinitely())` — blocks until GPU + mapping done
5. Read staging buffer, unpack voxels, check for all-air, insert into cache as `Option<BrickData>`

The cache is `Arc<Mutex<HashMap<[u32;3], Option<BrickData>>>>` — shared with worker threads via `GpuCachedSource`.

### `gpu_cached_source.rs` — BrickSource implementation

`GpuCachedSource` implements `BrickSource` with a three-tier lookup:
1. **GPU cache** — `HashMap::remove()` (takes ownership, O(1))
2. **Region disk cache** — delegates to `CachedSource<WorldGenerator>` (zstd decompress)
3. **CPU fallback** — if neither has data, falls back to CPU noise (should rarely happen)

Worker threads calling `generate()` hit the GPU cache for the vast majority of cells since `GpuWorldGenerator::generate_all()` pre-fills it before the pager starts.

### Integration in `scenes.rs`

`create_terrain_pager()` now:
1. Creates `GpuWorldGenerator` and calls `generate_all()` — fills the GPU cache
2. Creates `GpuCachedSource` wrapping the GPU cache + CPU fallback + disk cache
3. Creates `BrickPager` with the source
4. Calls `preload_all()` — worker threads drain the GPU cache into VRAM

The flow: GPU generates all voxel data (86ms) → pager workers pull from cache and upload to VRAM (140ms) → terrain fully loaded in ~226ms total.

## Performance

On Apple M1 Max, 32×12×32 grid (12288 cells, 7630 solid bricks):

| Path | Time |
|---|---|
| CPU procgen (first run) | 2534 ms |
| GPU procgen (first run) | **86 ms** |
| Region cache (second run) | 198 ms |

GPU generation is 29× faster than CPU and even beats the disk cache.

## Key decisions

1. **Blocking readback** — `generate_all()` blocks per batch. Acceptable during preload; a non-blocking `tick()` path can be added later for runtime streaming at larger world scales.
2. **Batch size 256** — balances dispatch overhead vs. staging buffer size (~1 MB per batch).
3. **No atomics for air detection** — instead, the readback loop checks if all 1024 words are zero. Simpler, no extra buffer.
4. **CPU fallback retained** — `GpuCachedSource` falls back to CPU noise if the GPU cache misses. Guarantees correctness even if GPU gen didn't cover a cell.
