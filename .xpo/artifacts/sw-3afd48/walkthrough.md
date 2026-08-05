## What was built

A CPU-side pooled allocator for 16³ voxel bricks, backed by two GPU storage buffers. This is the foundation data structure that every subsequent system (top-level index, DDA traversal, worldgen, streaming, editing) addresses voxels through.

## How the pieces fit together

### BrickHandle

An opaque handle with two fields: `slot` (u32 index into GPU buffers) and `generation` (CPU-side use-after-free guard). `gpu_index()` returns the raw slot for GPU addressing. `BrickHandle::NONE` (slot = `u32::MAX`) serves as a sentinel for empty index entries.

The generation is CPU-only — the GPU never sees it. When a slot is freed, its generation increments. Any handle still holding the old generation will fail `is_valid()` and trip a `debug_assert` on `free()` or `write_*`.

### BrickPool

Owns the GPU buffers and manages allocation state:

**GPU buffers:**
- `voxel_buf` — `capacity × 1024 u32s`. Each brick stores 16³ = 4096 voxels as 8-bit palette indices, packed 4-per-u32 (little-endian byte order). To read voxel `i` from brick `slot`: `(voxels[slot * 1024 + i/4] >> ((i%4) * 8)) & 0xFF`.
- `palette_buf` — `capacity × 256 u32s`. Each u32 is RGBA packed as `R | (G<<8) | (B<<16) | (A<<24)`. The shader unpacks to vec4<f32> by masking and dividing by 255.

Both buffers are `STORAGE | COPY_DST` — readable by compute shaders, writable by `queue.write_buffer`.

**CPU state:**
- `free_list: Vec<u32>` — stack of available slot indices. Initialized with all slots in reverse order so slot 0 is allocated first.
- `generations: Vec<u32>` — one counter per slot, incremented on each `free()`.
- `live_count: u32` — number of currently allocated bricks.

**Allocation is O(1):** `alloc()` pops from the free list, `free()` pushes back. No searching, no compaction.

### Data upload

`write_voxels` takes a `&[u8; 4096]` (one byte per voxel, x-fastest layout matching `brick_index()` in common.wgsl), packs into 1024 u32s, and writes to the correct offset in the voxel buffer.

`write_palette` takes a `&[[u8; 4]]` slice (RGBA per entry, up to 256 entries), packs each as a u32, and writes to the palette buffer. Partial palettes are fine — the unused entries are unspecified.

### Auxiliary channel extensibility

The handle design naturally supports future per-brick channels (fluid level/flow). Any new channel is just a buffer of `capacity × channel_size`, indexed by `handle.gpu_index()`. No changes to handles, pool, or existing buffers.

## Key decisions

- **Packed u8-as-u32 voxels** (4 KB/brick) over u32-per-voxel (16 KB/brick). One shift + one AND per lookup; 4x memory savings at scale.
- **Two separate buffers** (voxels + palettes) rather than interleaved. Keeps the traversal hot path (voxel reads) cache-friendly by not interleaving with palette data that's only touched on hit.
- **Fixed 256-entry palette** per brick rather than variable-length. Simplifies GPU indexing at the cost of ~1 KB/brick of mostly-unused palette space.

## What a future reader should know

- The pool doesn't own or know about the top-level spatial index — that's the next issue (sw-30aa3c). The pool is purely an allocator + GPU buffer manager.
- `BrickHandle::NONE` is used as the "empty" value in the top-level index. Any code building an index should initialize entries to `NONE`.
- The constants `BRICK_EDGE` (16) and `BRICK_VOLUME` (4096) are defined in both `brick_pool.rs` and `common.wgsl`. They must stay in sync.
