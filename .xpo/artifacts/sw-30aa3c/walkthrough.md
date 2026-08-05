## What was built

Replaced the dense 256³ raymarcher with a sparse brick-based system: a flat 3D grid index maps world-space brick coordinates to brick pool handles, and a two-level DDA compute shader skips empty grid cells (coarse) then traverses 16³ voxels inside occupied bricks (fine).

## How the pieces fit together

### BrickIndex (`crates/engine/src/brick_index.rs`)

A flat 3D grid of `u32` values stored in a GPU storage buffer. Each entry is either a brick pool slot index or `u32::MAX` (empty). A CPU-side `Vec<u32>` mirror supports construction via `set()`, then `upload()` pushes the full grid to the GPU in one `write_buffer` call.

The index also stores the world-space minimum corner (`world_min`) and exposes `brick_size()` (= `BRICK_EDGE × VOXEL_SCALE` = 1.6m). These feed into the shader uniform so the traversal maps between world space and grid space.

### Shader rewrite (`crates/engine/shaders/raymarch.wgsl`)

The shader now has 5 bindings: uniform (camera + world params), grid index, packed voxel data, palette data, and the output texture.

**Uniform layout** (128 bytes): The original 96-byte camera uniform was extended with `world_min` (vec3<f32>), `brick_size` (f32), and `grid_dims` (vec3<u32>).

**Two-level DDA:**

1. **Coarse DDA** — steps through grid cells, each cell = one brick (1.6m). For each cell, looks up `grid_map[flat_index]`. If `== 0xFFFFFFFF`, the cell is empty — skip immediately (the core performance primitive). If valid, enters the fine DDA.

2. **Fine DDA** — traces through the 16³ voxels of the occupied brick. Reads voxels from the packed u32 array: `(voxels[handle * 1024 + idx/4] >> ((idx%4)*8)) & 0xFF`. On non-zero hit, looks up the per-brick palette: unpacks RGBA from `palettes[handle * 256 + material]`.

**Entry normal tracking:** The coarse DDA tracks which face the ray entered through (`coarse_normal`). This is passed to `trace_brick` as the initial normal for the fine DDA. Without this, voxels at brick boundaries would have `normal = (0,0,0)` and render with ambient-only shading — visible as dark rings on the sphere (caught and fixed during review).

**Traversal loop structure:** `throughput` and `accumulation` variables are present in the main loop for future water transmission and fog/god-ray accumulation. They're unused today; the compiler eliminates the dead code.

### Raymarcher rewrite (`crates/engine/src/raymarcher.rs`)

No longer owns voxel data. Takes `BrickPool` and `BrickIndex` by reference during `new()` and `resize()`. The compute bind group references the pool's voxel/palette buffers and the index's buffer — wgpu bind groups hold `Arc` refs internally, so the buffers stay alive.

`render()` takes a `&BrickIndex` for the world parameters (world_min, brick_size, grid_dims) that go into the uniform buffer each frame.

### Viewer changes (`crates/viewer/src/main.rs`)

Creates `BrickPool(4096)` and `BrickIndex([16,16,16], Vec3::splat(-12.8))` during startup. `generate_test_world()` iterates over all 4,096 grid cells, generates voxels per brick using the same sphere+ground formula as the dense version, and only allocates bricks that contain solid content. Result: 1,423 bricks allocated (35% of grid cells).

Debug panel shows `Bricks: 1423 / 4096`.

## Key decisions

- **Flat 3D grid** rather than a hash map or tree. 16³ = 4,096 entries × 4 bytes = 16 KB. Even at scale (256³ = 16M cells = 64 MB), a flat grid is simpler and faster than pointer-chasing alternatives.

- **`grid_map` not `brick_index`** as the WGSL variable name. `common.wgsl` already defines a `brick_index()` function, and they're composed together via `shaders::compose`.

- **Coarse normal passed to fine DDA** as `entry_normal`. Prevents ambient-only shading on voxels at brick face boundaries.

## What a future reader should know

- The `generate_test_world` function in `main.rs` is a placeholder — it produces the same sphere+ground scene as the dense spike. The real worldgen is issue sw-00ae86.

- The grid is world-axis-aligned with a fixed origin. Per-instance transforms (for voxel objects) are a separate system (sw-b254c8, Scene Structure epic).

- The shader's `WORDS_PER_BRICK` and `PALETTE_ENTRIES` constants must match the Rust-side `brick_pool` constants. They're currently duplicated — a future cleanup could have the shader read them from a uniform or const buffer.
