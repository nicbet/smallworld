
# Async Brick Paging with Residency Tracking & Memory Budget

## What

An async paging system that streams brick data into the GPU on demand, driven by camera position and SSE. The engine provides a `BrickSource` trait and a `BrickPager` orchestrator. Games implement their own sources — procedural, disk-cached, networked, whatever. The sandbox implements a `CachedSource` with a region file format for fast repeated loads.

## Why

The current system generates and uploads all bricks synchronously at startup. This blocks for hundreds of milliseconds on a 32×12×32 grid and cannot scale to km-scale worlds. We need to:

1. Load only what's visible, evict what isn't
2. Never hitch during flight
3. Let games control where brick data comes from

## Boundary

**Engine** (library, reusable):
- `BrickSource` trait
- `BrickData` struct
- `BrickPager` orchestrator (demand, async pipeline, residency, eviction)
- `BrickPool` / `BrickIndex` extensions (`reassign`, `clear_cell`)

**Sandbox** (application, game-specific):
- `WorldGenerator` as a `BrickSource` impl
- `CachedSource` wrapper (disk-first with write-back)
- Region file format (`.swr` files)
- Cache directory management

## How — Engine

### `brick_data.rs` — shared data types

```rust
pub struct BrickData {
    pub voxels: [u8; 4096],
    pub palette: Vec<[u8; 4]>,
}
```

### `brick_source.rs` — data provider trait

```rust
pub trait BrickSource: Send + Sync {
    /// Generate or load brick data for the given grid cell.
    /// Called on background threads — no GPU access.
    /// Returns None for empty cells (air).
    fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData>;
}
```

### `brick_pager.rs` — orchestrator

```rust
pub struct PagerConfig {
    pub max_uploads_per_frame: u32,   // default 64
    pub worker_threads: usize,        // default 2
}

pub struct PagerStats {
    pub resident: u32,
    pub mip_only: u32,
    pub loading: u32,
    pub empty: u32,
    pub evicted_this_frame: u32,
    pub uploaded_this_frame: u32,
}

pub struct BrickPager { /* ... */ }
```

Owns the async pipeline and residency state. Created with a `BrickPool`, grid dims, and an `Arc<dyn BrickSource>`.

### Per-cell state machine

```
Empty ──→ Loading ──→ Resident ──→ MipOnly ──→ Empty
                         ↑            │
                         └────────────┘
                        (re-demand)
```

- **Empty**: no slot assigned, grid cell = `u32::MAX`
- **Loading**: load request in flight, no slot yet
- **Resident**: slot assigned, voxels + palette + mips all valid
- **MipOnly**: slot assigned, mips valid, voxels/palette stale. Shader uses mips via SSE check (already implemented).
- **MipOnly → Empty**: slot reclaimed for a different cell (full eviction)

### Demand computation

Each frame, `BrickPager::update()` walks the grid and classifies cells by SSE from the camera:

1. **Needed** — `sse ≥ threshold` AND state is Empty or MipOnly → enqueue load request
2. **Warm** — `sse ≥ threshold` AND state is Resident → touch LRU timestamp
3. **Cold** — `sse < threshold` AND state is Resident → transition to MipOnly
4. **Far/invisible** — too far to matter → no action

SSE per cell:

```
cell_center = world_min + (grid_pos + 0.5) * brick_size
dist = length(cell_center - camera_pos)
sse = brick_size * focal_length / dist
```

Same formula the shader uses, so demand and rendering agree.

### Async pipeline

```
Main thread                    Worker threads (N=2)
    │                               │
    ├─ enqueue LoadRequest ────────→│
    │   (grid_pos, world_min)       │
    │                               ├─ source.generate()
    │                               │   (whatever the game does)
    │←─── LoadResult ──────────────┤
    │   (grid_pos, BrickData)       │
    │                               │
    ├─ upload to GPU (capped)       │
```

- **Channels**: `crossbeam-channel` bounded MPMC. Request channel bounded to `pool_capacity / 2`. Result channel unbounded.
- **Workers**: `std::thread::spawn`, long-lived, block on request channel recv.
- **Deduplication**: `HashSet<[u32; 3]>` of in-flight grid positions prevents duplicate loads.
- **Priority**: requests sorted by SSE descending (closest first) via `BinaryHeap`.

### Per-frame upload

Main thread drains the result channel and uploads up to `max_uploads_per_frame` bricks:

1. Pop completed `LoadResult`
2. If result is `None` (air cell) → mark cell Empty, remove from in-flight set, skip
3. Allocate slot:
   - Try `pool.alloc()` (free list)
   - If exhausted → find coldest MipOnly slot (lowest `last_used` frame), reclaim it:
     - Set old cell's grid entry to `u32::MAX`, mark old cell Empty
     - Reassign the slot via `pool.reassign(slot)`
   - If no MipOnly slots available → drop the result, retry next frame
4. Upload: `pool.write_voxels()`, `pool.write_palette()`, `pool.write_mips()`
5. `index.set(grid_pos, handle)` — update grid cell
6. Mark cell Resident, record `last_used = current_frame`

Batch `index.upload()` once at the end of all per-frame uploads.

### BrickPool changes

Add `reassign()` — reuses an existing slot without free/alloc round-trip:

```rust
pub fn reassign(&mut self, slot: u32) -> BrickHandle {
    self.generations[slot as usize] = self.generations[slot as usize].wrapping_add(1);
    BrickHandle { slot, generation: self.generations[slot as usize] }
}
```

### BrickIndex changes

Add `clear_cell()`:

```rust
pub fn clear_cell(&mut self, pos: [u32; 3]) {
    let idx = self.flat_index(pos);
    self.data[idx] = u32::MAX;
}
```

### New engine dependency

`crossbeam-channel` — lock-free bounded/unbounded MPMC channels.

## How — Sandbox

### `WorldGenerator` as `BrickSource`

Thin adapter — `generate()` calls the existing noise-based terrain gen.

### Region file format (`.swr`)

A **region** covers a 16×16×16 cube of grid cells. One file per region.

**File layout:**

```
┌─────────────────────────────────────────────┐
│ Header (16 KB)                              │
│   4096 entries × 4 bytes each               │
│   Each entry: offset (24 bits) + size (8 bits) │
│   offset = sector number (× 4096 bytes)     │
│   size = sector count                       │
│   entry (0, 0) = confirmed empty (air)      │
├─────────────────────────────────────────────┤
│ Sectors (4 KB each)                         │
│   Each brick payload:                       │
│     4 bytes: payload length (little-endian) │
│     N bytes: zstd-compressed BrickData      │
│       - 4096 bytes voxels                   │
│       - 1 byte palette_len                  │
│       - palette_len × 4 bytes RGBA          │
│     Padded to sector boundary               │
└─────────────────────────────────────────────┘
```

**API:**

```rust
pub struct RegionFile { /* buffered file handle */ }

impl RegionFile {
    pub fn open_or_create(path: &Path) -> io::Result<Self>;
    pub fn read_brick(&self, local_pos: [u32; 3]) -> io::Result<Option<BrickData>>;
    pub fn write_brick(&mut self, local_pos: [u32; 3], data: &BrickData) -> io::Result<()>;
}
```

- `local_pos` is position within the 16³ region (0..16 each axis)
- Region file path: `regions/r.{rx}.{ry}.{rz}.swr`
- Coordinate mapping: `region_pos = grid_pos / 16`, `local_pos = grid_pos % 16`

### `CachedSource<S: BrickSource>`

Wraps any `BrickSource`:
1. Check region file → **hit**: decompress and return
2. **Miss**: call `inner.generate()` → if `Some`, compress and write to region → return
3. Air cells written as offset=0/size=0 in header to avoid re-generating

Cache location: `~/.cache/smallworld/sandbox/{preset_name}/regions/`

### Sandbox new dependency

`zstd` — fast compression for region payloads. ~3-5× ratio on voxel data, decompression >1 GB/s.

### Sandbox integration

- Terrain presets create `CachedSource<WorldGenerator>` and hand it to `BrickPager`
- `Preset::setup()` no longer generates terrain synchronously — creates the pager instead
- Scene objects (trees, rocks) still load synchronously (instanced, small, needed immediately)
- Debug panel gains "Pager" section: resident / mip-only / loading / empty, uploads/evictions per frame
- `--bench` works with streaming pager

## Decisions

1. **Region format is sandbox code, not engine** — engine provides the paging primitives; how games persist/cache data is their concern.
2. **Region size 16³** — matches brick edge length, clean coordinate math. 4096 cells per region, 16 KB header.
3. **Sector-based layout** — 4 KB sectors, same approach as Minecraft. Enables in-place updates without rewriting.
4. **zstd** — fast, good ratio. Sandbox dep only.
5. **crossbeam-channel** — lock-free MPMC, engine dep. Significantly faster than `std::sync::mpsc` under contention.
6. **MipOnly keeps the slot** — Resident → MipOnly does NOT free the slot. Mips stay valid for distant rendering. Slot only reclaimed when pool is exhausted.
7. **No frustum culling for demand** — at 12K cells, walking all cells is trivial. Future optimization if grid grows past ~100K.
8. **Per-frame upload cap 64** — 64 × ~6.3 KB ≈ 400 KB/frame. Well within GPU upload bandwidth. Keeps spikes < 2ms.
9. **Air cell caching** — header entry (0,0) means confirmed air. Prevents re-generating empty cells on every load.
10. **Priority by distance** — closest bricks load first for best visual pop-in behavior.

## Acceptance criteria

### Engine
- [ ] `BrickSource` trait and `BrickData` struct
- [ ] `BrickPager` with `crossbeam-channel` worker threads and per-frame upload cap
- [ ] Residency state machine: Empty → Loading → Resident → MipOnly → Empty
- [ ] LRU eviction when pool exhausted — coldest MipOnly brick reclaimed
- [ ] Per-frame upload cap prevents hitches (< 2ms upload time per frame)
- [ ] `BrickPool::reassign()` and `BrickIndex::clear_cell()`

### Sandbox
- [ ] `WorldGenerator` implements `BrickSource`
- [ ] Region file format with zstd compression
- [ ] `CachedSource` wrapper: disk-first, generate-on-miss, write-back
- [ ] Default and TerrainOnly presets use paged loading via `CachedSource`
- [ ] Second launch of same preset skips generation — loads from region cache
- [ ] Debug panel shows pager stats (resident/mip-only/loading/empty, uploads/evictions)
- [ ] `--bench` works with streaming pager
- [ ] No visual difference from synchronous loading once all visible bricks are resident
