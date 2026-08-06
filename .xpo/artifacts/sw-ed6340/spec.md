
# GPU Compute Noise Generation

## What

A compute shader that generates brick voxel data from the same noise function as `worldgen.rs`, dispatched in batches on the main thread. Results are read back asynchronously and cached in a concurrent map. The existing `BrickSource` worker threads pull from this cache instead of evaluating CPU noise.

## Why

CPU procgen takes ~1ms/brick. At 262K cells, that's 4+ minutes before the paging system can be tested. GPU compute can evaluate thousands of bricks per dispatch — the noise math is embarrassingly parallel (4096 independent voxels per brick, each doing the same fbm3d).

## Boundary

**Engine**: untouched. `BrickSource`, `BrickPager`, `BrickData` — all unchanged.

**Sandbox**: new compute shader, new `GpuWorldGenerator` source, updated presets.

## How

### Compute shader: `crates/sandbox/shaders/worldgen.wgsl`

Implements the same noise pipeline as `worldgen.rs`:
- `hash()`, `noise3d()`, `fbm3d()` — direct port of the CPU functions
- `sample(wx, wy, wz)` → material index (same density/cave/strata logic)
- Entry point: `@workgroup_size(4, 4, 4)` — one workgroup fills one 16³ brick (256 threads, each fills a 4×4×4 sub-block = 64 voxels each, though exact mapping is flexible)

**Bindings:**
- `@group(0) @binding(0) var<uniform> params: GenParams` — seed, world_min, brick_size, grid_dims, batch offset
- `@group(0) @binding(1) var<storage, read> requests: array<vec4<u32>>` — grid positions to generate (xyz + padding)
- `@group(0) @binding(2) var<storage, read_write> output: array<u32>` — packed voxel data, 1024 u32s per brick (4 voxels per u32)

Each thread computes its local voxel coordinates from `workgroup_id` (which brick) and `local_invocation_id` (which voxels within the brick), evaluates `sample()`, and writes to the output buffer.

### `GpuWorldGenerator` (sandbox module)

```rust
pub struct GpuWorldGenerator {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    param_buf: wgpu::Buffer,
    request_buf: wgpu::Buffer,
    output_buf: wgpu::Buffer,
    staging_buf: wgpu::Buffer,      // MAP_READ for async readback
    cache: Arc<DashMap<[u32; 3], Option<BrickData>>>,
    batch_size: u32,                 // bricks per dispatch (e.g. 256)
    pending_readback: VecDeque<PendingBatch>,
}
```

**Per-frame flow (main thread, non-blocking):**

1. **Check demand** — the pager's demand computation identifies Unknown cells. A shared queue (or the pager exposes pending requests) tells the GPU gen what to generate next.
2. **Dispatch** — write grid positions to `request_buf`, dispatch compute shader for up to `batch_size` bricks.
3. **Copy + map** — `encoder.copy_buffer_to_buffer(output_buf, staging_buf)`, then `staging_buf.slice(..).map_async(MapMode::Read, callback)`. Push to `pending_readback`.
4. **Poll completions** — check `pending_readback` for mapped buffers. When ready, read voxel data, build `BrickData` (with the shared static palette), insert into `cache`.

**No blocking on the main thread** — `map_async` is a callback, `device.poll(Maintain::Poll)` drives it.

### `GpuCachedSource` — BrickSource implementation

Wraps the GPU cache + `CachedSource<WorldGenerator>` fallback:

```rust
impl BrickSource for GpuCachedSource {
    fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
        // 1. Check GPU results cache
        if let Some(entry) = self.gpu_cache.remove(&grid_pos) {
            // GPU already generated it — use result, write to region cache
            self.region_cache.write_through(grid_pos, entry.as_ref());
            return entry;
        }
        // 2. Check region file cache (previous run)
        if let Some(data) = self.region_cache.try_read(grid_pos) {
            return data;
        }
        // 3. CPU fallback (cold path — GPU hasn't gotten here yet)
        let data = self.cpu_gen.generate_brick(grid_pos, world_min)
            .map(|gb| BrickData { voxels: gb.voxels, palette: gb.palette.to_vec() });
        self.region_cache.write_through(grid_pos, data.as_ref());
        data
    }
}
```

Worker threads never block — they check the GPU cache (concurrent map, O(1)), fall back to disk cache, then CPU noise as last resort. In practice, the GPU runs far ahead of demand so most hits come from the GPU cache.

### Demand coordination

The pager already computes demand (sorted by priority). We need the GPU gen to know what to generate next. Options:

**Simple**: GPU gen maintains its own "generation frontier" — walks the grid in a spiral from the camera start, generates everything. Doesn't need to coordinate with the pager's demand. The GPU is fast enough that it generates everything long before the pager asks for it.

**Coordinated**: Pager exposes its demand queue. GPU gen consumes from it. More complex, better prioritization.

Start with **simple** (spiral walk). The GPU generates all cells in order, regardless of camera. At 256 bricks/dispatch and ~1ms/dispatch, 262K cells = ~1024 dispatches = ~1 second for the entire world. By the time the camera moves, everything is already in the cache.

### Palette handling

The GPU shader writes material indices (same as CPU). The palette is a constant (`PALETTE` in worldgen.rs). `BrickData` is assembled on readback with the static palette. No GPU palette generation needed.

### Dependencies

`DashMap` or similar concurrent hashmap — or use `Arc<Mutex<HashMap>>` (simpler, contention is low since GPU cache writes are batched and reads are fast).

Start with `Arc<Mutex<HashMap>>` to avoid a new dep.

## Decisions

1. **Shader in sandbox, not engine** — worldgen is game logic
2. **Spiral walk, not demand-coordinated** — GPU is fast enough to generate everything; simpler implementation
3. **Same noise functions** — direct port ensures identical terrain
4. **Static palette** — GPU only writes material indices, palette attached on readback
5. **`Arc<Mutex<HashMap>>`** — avoids dashmap dep; contention is minimal (batched writes, fast reads)
6. **CPU fallback** — worker threads use CPU noise if GPU hasn't reached that cell yet; guarantees correctness

## Acceptance criteria

- [ ] Compute shader generates identical voxel data to CPU worldgen (verified by test)
- [ ] `GpuWorldGenerator` dispatches batches, reads back asynchronously
- [ ] `GpuCachedSource` implements `BrickSource`: GPU cache → disk cache → CPU fallback
- [ ] Main thread dispatch + readback adds < 1ms/frame
- [ ] 128×16×128 grid (262K cells) generates in < 5 seconds on M1 Pro
- [ ] Terrain presets updated to use GPU generation
- [ ] Region cache still works (GPU results written through to disk)
- [ ] Visual output identical to CPU-generated terrain (same seed → same world)
