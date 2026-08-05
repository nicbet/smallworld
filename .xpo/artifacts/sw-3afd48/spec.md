## What

CPU-side pooled allocator for 16³ voxel bricks backed by GPU storage buffers. Each brick stores 4 KB of voxel data (8-bit palette indices packed 4-per-u32) and a 1 KB per-brick material palette (256 × RGBA32). Stable handles with generation-based validation; free-list recycling.

## Why

The brick pool is the foundation data structure for the sparse world. Every subsequent system — the top-level index, the DDA traversal shader, worldgen, streaming, editing — addresses voxels through brick handles into this pool. Getting the layout and handle semantics right here avoids rework downstream.

## Acceptance Criteria

- `BrickPool::new(device, capacity)` pre-allocates two GPU storage buffers (voxels + palettes)
- `alloc()` returns a `BrickHandle` from the free list; `free(handle)` recycles the slot
- Generation counter on each slot detects use-after-free (panics in debug, no-ops in release)
- `write_voxels` and `write_palette` upload data to the correct slot via `queue.write_buffer`
- Accessors expose the GPU buffers for shader binding
- Unit tests cover: alloc/free round-trip, free-list reuse, generation validation, capacity exhaustion
- `cargo clippy` and `cargo test` pass

## Flow

### 1. New module — `crates/engine/src/brick_pool.rs`

**`BrickHandle`** — opaque handle to a live brick:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BrickHandle {
    slot: u32,
    generation: u32,
}
```

- `gpu_index() -> u32` — returns the slot index for GPU buffer addressing
- `BrickHandle::NONE` — sentinel value (slot = `u32::MAX`) for empty index entries

**`BrickPool`** — owns the GPU buffers and manages allocation:

```rust
pub struct BrickPool {
    voxel_buf: wgpu::Buffer,     // capacity × 1024 u32s (4 KB per brick)
    palette_buf: wgpu::Buffer,   // capacity × 256 u32s (1 KB per brick)
    generations: Vec<u32>,       // per-slot generation, increments on free
    free_list: Vec<u32>,         // stack of available slot indices
    capacity: u32,
    live_count: u32,
}
```

**Constants:**

| Constant | Value | Rationale |
|----------|-------|-----------|
| `BRICK_EDGE` | 16 | From `common.wgsl` |
| `BRICK_VOLUME` | 4096 | 16³ |
| `VOXELS_PER_WORD` | 4 | 8-bit voxels packed into u32 |
| `WORDS_PER_BRICK` | 1024 | 4096 / 4 |
| `PALETTE_ENTRIES` | 256 | 8-bit palette index range |

**Public API:**

- `new(device: &Device, capacity: u32) -> Self`
  Pre-allocates `voxel_buf` (capacity × 4 KB) and `palette_buf` (capacity × 1 KB) as `STORAGE | COPY_DST` buffers. Initializes free list with all slots in reverse order (so slot 0 is allocated first).

- `alloc() -> Option<BrickHandle>`
  Pops a slot from the free list, returns handle with current generation. Returns `None` if exhausted.

- `free(&mut self, handle: BrickHandle)`
  Validates generation match (debug_assert). Increments the slot's generation. Pushes slot back onto the free list. Decrements live count.

- `write_voxels(&self, queue: &Queue, handle: BrickHandle, data: &[u8; BRICK_VOLUME])`
  Packs 4096 bytes into 1024 u32s, writes to the correct offset in `voxel_buf`.

- `write_palette(&self, queue: &Queue, handle: BrickHandle, entries: &[[u8; 4]])`
  Writes RGBA entries to the palette buffer. Slice length ≤ 256; unused entries are unspecified.

- `voxel_buffer(&self) -> &Buffer` / `palette_buffer(&self) -> &Buffer`
  For shader bind group creation.

- `capacity(&self) -> u32` / `live_count(&self) -> u32`
  Stats for debug overlay.

### 2. GPU buffer layout

**Voxel buffer** — `array<u32>`, indexed as `slot * 1024 + (voxel_idx / 4)`:

```
┌─ slot 0 ─┐┌─ slot 1 ─┐┌─ slot 2 ─┐
│ 1024 u32s ││ 1024 u32s ││ 1024 u32s │ ...
└───────────┘└───────────┘└───────────┘
```

Shader reads: `(voxels[slot * 1024 + voxel_idx / 4] >> ((voxel_idx % 4) * 8)) & 0xFF`

**Palette buffer** — `array<u32>`, indexed as `slot * 256 + material_idx`:

```
┌─ slot 0 ──┐┌─ slot 1 ──┐
│ 256 u32s   ││ 256 u32s   │ ...  (each u32 = RGBA packed)
└────────────┘└────────────┘
```

Shader reads: unpack u32 → vec4<f32> by shifting and dividing by 255.

### 3. Register module — `crates/engine/src/lib.rs`

Add `pub mod brick_pool`.

### 4. No viewer changes

The pool isn't wired into the raymarcher yet — that's sw-30aa3c (top-level index + DDA).

## Decisions

**D1: Packed u8-as-u32 voxels (4 KB/brick), not u32-per-voxel (16 KB/brick).**
4x memory reduction. The bit manipulation (one shift + one AND per lookup) is trivial compared to the memory bandwidth saved. At scale (500K bricks), this is 2 GB vs 8 GB.

**D2: Fixed 256-entry palette per brick (1 KB), not variable-length.**
Simplifies GPU indexing (no indirection table). Most bricks use <10 entries, but the unused entries cost only ~1 KB/brick and avoid a second level of indirection on every voxel hit.

**D3: RGBA32 palette entries (4 bytes each), not vec4<f32> (16 bytes each).**
4x smaller palettes. The shader unpacks to float on read — one ALU operation, negligible vs the memory access.

**D4: Generation-based handle validation, not arena-style indices.**
Detects use-after-free at the cost of 4 bytes per handle and one u32 comparison per operation. Panics in debug, can be compiled out in release.

**D5: Handle `gpu_index()` returns a raw slot index — no indirection.**
The GPU sees brick handles as plain u32 offsets. This means freed-and-reallocated slots reuse the same GPU index, which is correct since the generation is CPU-only.

## Auxiliary channel extensibility

The design must not preclude attaching side-channel data (fluid level/flow) to a brick later. This is satisfied by the handle design: `BrickHandle::gpu_index()` can index into any per-brick buffer. Adding an auxiliary channel is just allocating a new buffer of `capacity × channel_size` and indexing with the same slot. No changes to handles, pool, or existing buffers needed.

## Assumptions

- Initial capacity of 4096 bricks (~20 MB total) is sufficient for the test world in the next issue
- `max_storage_buffer_binding_size` (128 MB default) accommodates the voxel buffer at this capacity
- The packing/unpacking cost of 4-per-u32 voxels is negligible compared to memory access latency
