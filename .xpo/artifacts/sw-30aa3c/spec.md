## What

Replace the dense 256³ raymarcher with a sparse brick-based system: a flat 3D grid index mapping world-space brick coordinates to `BrickHandle` slots, and a two-level DDA compute shader (coarse through grid cells, fine inside 16³ bricks). The same test scene (ground + sphere) renders identically but only allocates bricks that contain solid voxels.

## Why

The dense 256³ volume doesn't scale. A 4 km world at 16-voxel bricks is ~2,500 bricks per axis — most of them empty air. The sparse index + hierarchical DDA is the core performance primitive (DESIGN.md §4): empty-space skipping eliminates the vast majority of traversal work.

## Acceptance Criteria

- A 16×16×16 brick grid renders the same test scene (ground at y<80, sphere radius 40) using the brick pool
- Only bricks containing solid voxels are allocated (~1,400 of 4,096 grid cells)
- Empty grid cells are skipped by the coarse DDA (no per-voxel work)
- Camera fly-around works at interactive frame rates
- egui debug overlay and frame time graph still render on top
- The traversal loop has explicit `throughput`/`accumulation` variables for future water/fog hooks
- `cargo clippy` and `cargo test` pass

## Flow

### 1. New module — `crates/engine/src/brick_index.rs`

`BrickIndex` — flat 3D grid of u32 brick handles on the GPU:

```rust
pub struct BrickIndex {
    buffer: wgpu::Buffer,      // STORAGE | COPY_DST
    data: Vec<u32>,            // CPU mirror (u32::MAX = empty)
    dims: [u32; 3],
    world_min: glam::Vec3,
}
```

**Public API:**
- `new(device, dims, world_min)` — allocates buffer, fills with `u32::MAX`
- `set(&mut self, pos: [u32; 3], handle: BrickHandle)` — write to CPU mirror
- `get(&self, pos: [u32; 3]) -> u32` — read from CPU mirror
- `upload(&self, queue)` — bulk-upload CPU mirror to GPU
- `buffer()`, `dims()`, `world_min()`, `brick_size() -> f32` — accessors

### 2. Rewrite shader — `crates/engine/shaders/raymarch.wgsl`

Composed with `common.wgsl`. New uniform layout (128 bytes):

```wgsl
struct Uniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    resolution: vec2<f32>,
    _pad0: vec2<f32>,
    world_min: vec3<f32>,
    brick_size: f32,
    grid_dims: vec3<u32>,
    _pad1: u32,
}
```

**Bindings (group 0):**
| Binding | Type | Contents |
|---------|------|----------|
| 0 | uniform | `Uniforms` |
| 1 | storage, read | Brick index grid (`array<u32>`) |
| 2 | storage, read | Voxel data from brick pool (`array<u32>`, packed u8) |
| 3 | storage, read | Palette data from brick pool (`array<u32>`, RGBA32) |
| 4 | storage_texture, write | Output image |

**Two-level DDA:**

1. Ray generation (same as before: inverse VP → world ray)
2. Ray-world AABB test against the grid bounds
3. **Coarse DDA** through grid cells (each cell = one brick = `brick_size` in world units):
   - Look up `brick_index[grid_flat_index]`
   - If `== 0xFFFFFFFF`: skip (empty space — the perf primitive)
   - If valid: enter fine DDA
4. **Fine DDA** inside the 16³ brick:
   - Read voxel: `(voxels[handle * 1024 + idx/4] >> ((idx%4)*8)) & 0xFF`
   - On non-zero hit: read palette `palettes[handle * 256 + material]`, unpack RGBA, shade
   - On miss: return to coarse DDA
5. On hit: shade with face normal + sun direction (same shading as before)
6. On all miss: sky gradient

**Loop structure for future extensibility:**

```wgsl
var throughput = vec3(1.0);   // future: water transmission
var accumulation = vec3(0.0); // future: fog/god ray accumulation
```

These are present in the loop but only used structurally — the compiler optimizes them away. Hooks for water and fog are marked with comments.

### 3. Rewrite — `crates/engine/src/raymarcher.rs`

The `Raymarcher` no longer owns voxel data. It takes the brick pool and index by reference:

```rust
pub fn new(gpu, width, height, surface_format, pool: &BrickPool, index: &BrickIndex) -> Self
pub fn resize(&mut self, gpu, width, height, pool: &BrickPool, index: &BrickIndex)
pub fn render(&self, gpu, encoder, surface_view, camera, index: &BrickIndex, compute_ts, blit_ts)
```

The compute bind group references the pool's voxel/palette buffers and the index's buffer. Bind groups are rebuilt on resize (output texture changes); pool/index buffers are stable.

Uniform buffer grows from 96 to 128 bytes to include `world_min`, `brick_size`, `grid_dims`.

### 4. Register module — `crates/engine/src/lib.rs`

Add `pub mod brick_index`.

### 5. Viewer integration — `crates/viewer/src/main.rs`

- Create `BrickPool` (capacity 4096) and `BrickIndex` (dims [16,16,16], world_min = (-12.8, -12.8, -12.8))
- `generate_test_world()` iterates over all grid cells, generates voxels per brick, allocates only bricks with solid content
- Pass pool + index to `Raymarcher::new()`
- Update `resize()` and `render()` calls
- Add brick count to debug panel: `Bricks: 1,423 / 4,096`

### 6. Shader enum

Add `Shader::Raymarch` already exists; the file content is replaced. No enum changes needed.

## Decisions

**D1: Flat 3D grid, not a tree or hash map.**
A 16×16×16 grid = 4,096 u32 entries = 16 KB. Even at 256³ grid cells = 64 MB, a flat grid is simpler and faster than a tree (single array lookup vs. pointer chasing). The shallow tree optimization is deferred until the world size demands it.

**D2: CPU mirror of the grid for construction, bulk upload.**
Building the grid on the CPU with `set()` + `upload()` is simpler than individual `write_buffer` calls per cell. The mirror is also useful for future CPU-side queries (collision, streaming decisions).

**D3: Raymarcher borrows pool/index buffers, doesn't own them.**
The pool and index outlive the raymarcher's bind groups. wgpu bind groups internally hold `Arc` refs to buffers, so the bind groups keep the buffers alive. The raymarcher only needs the pool/index during `new()` and `resize()`.

**D4: Combined camera + world uniform (128 bytes), not separate UBOs.**
One uniform buffer, one bind group entry. The 32-byte overhead is negligible; the simplicity is worth it.

**D5: `throughput` and `accumulation` variables present but unused.**
The traversal loop is shaped for future water (transmission segment) and fog (per-step accumulation) without those features being implemented. The shader compiler eliminates dead code.

## Assumptions

- 16×16×16 grid = 4,096 cells, of which ~1,400 contain solid voxels for the test scene
- Brick pool capacity of 4,096 is sufficient
- The same camera position (0, 2, 5) and sphere/ground scene produces a visually identical result
