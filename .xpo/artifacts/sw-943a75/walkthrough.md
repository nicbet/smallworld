
# Async Brick Paging — Implementation Walkthrough

## What was built

A two-layer streaming system that loads voxel bricks into the GPU on demand, driven by camera position and screen-space error (SSE). The engine provides the paging primitives; the sandbox implements disk-backed caching on top.

## Architecture

### Engine layer

Three new modules in `crates/engine/src/`:

**`brick_data.rs`** — A simple struct holding `voxels: [u8; 4096]` and `palette: Vec<[u8; 4]>`. This is the data unit that crosses the source→pager boundary.

**`brick_source.rs`** — The `BrickSource` trait with a single method `generate(grid_pos, world_min) -> Option<BrickData>`. Called on background threads, so it must be `Send + Sync` and must not touch the GPU. Returns `None` for air cells.

**`brick_pager.rs`** — The core orchestrator. Key pieces:

1. **Cell state machine** — each grid cell is in one of five states:
   - `Unknown` — never loaded, eligible for requests
   - `Air` — confirmed empty, never re-requested
   - `Loading { existing_slot }` — request in flight, optionally carrying a slot from a prior MipOnly state
   - `Resident { slot }` — full data in VRAM
   - `MipOnly { slot }` — only mip data valid, eviction candidate

2. **Demand computation** — each frame, walks all grid cells and computes SSE from camera distance. Cells above threshold that aren't Resident get enqueued. Cells below threshold that are Resident transition to MipOnly. Requests are sorted by SSE descending (closest first) before dispatch.

3. **Async pipeline** — two worker threads (configurable) consume `[u32; 3]` grid positions from a bounded `crossbeam-channel` (cap 256), call `source.generate()`, and send results back on an unbounded channel. Deduplication via `HashSet<[u32; 3]>` prevents duplicate in-flight requests.

4. **Per-frame upload** — drains up to 64 results per frame. Each upload: allocate a pool slot (free list → evict coldest MipOnly → drop if nothing available), write voxels + palette + mips to GPU, update the grid index. Batch `index.upload()` once at the end.

5. **LRU eviction** — when the pool is exhausted, `find_coldest_mip_only()` scans all slots for the one with the oldest `last_used` frame that's in MipOnly state. Its cell reverts to Unknown (so it can be re-loaded later), and the slot is reassigned to the new brick.

**BrickPool changes** — added `reassign(slot) -> BrickHandle` which bumps the generation and returns a valid handle without a free/alloc round-trip.

**BrickIndex changes** — added `clear_cell(pos)` which sets a grid cell back to `u32::MAX` (empty).

### Sandbox layer

Two new modules in `crates/sandbox/src/`:

**`region.rs`** — Minecraft-inspired region file format. Each `.swr` file stores a 16×16×16 cube of grid cells:
- 16 KB header (4096 entries × 4 bytes: 24-bit sector offset + 8-bit sector count)
- 4 KB sectors with zstd-compressed brick payloads (voxels + palette_len + palette RGBA)
- Air cells marked with a sentinel (offset=0, size=0xFF) so they aren't re-generated
- Files live at `~/.cache/smallworld/sandbox/{preset}/regions/r.{rx}.{ry}.{rz}.swr`

**`cached_source.rs`** — `CachedSource<S: BrickSource>` wraps any source with disk caching:
- On `generate()`: check if the region file has an entry → hit: decompress and return → miss: delegate to inner source, write result back to region file, return
- Region files are lazily opened via `RwLock<HashMap<..., Arc<Mutex<RegionFile>>>>`
- `cache_dir_for_preset()` helper resolves the platform-appropriate cache directory

### Integration

**`worldgen.rs`** — `WorldGenerator` now implements `BrickSource`. Internal struct renamed from `BrickData` to `GeneratedBrick` to avoid collision with the engine type. The adapter copies the static palette to a Vec.

**`scenes.rs`** — `Preset::setup()` now returns `Option<BrickPager>`. Default and TerrainOnly presets create a `CachedSource<WorldGenerator>` and hand it to `BrickPager::new()`. Object placement still happens synchronously (objects use `WorldGenerator::find_surface_y()` for terrain surface queries — this is a pure noise evaluation, independent of loaded bricks).

**`main.rs`** — `RunState` gains `pager: Option<BrickPager>` and `pager_stats: PagerStats`. Each frame, before rendering, calls `pager.update()` with camera position, focal length, and SSE threshold. Debug panel shows pager stats (resident, mip-only, loading, air, unknown, uploads/frame, evictions/frame).

## Key design decisions

1. **SSE alignment** — the pager computes SSE the same way the shader does (`brick_size * focal_length / dist`). This means the demand computation and the rendering agree on which bricks need full-res data vs. mips. A brick that the shader would use mips for is also a brick the pager considers cold.

2. **MipOnly ≠ evicted** — transitioning to MipOnly doesn't free the slot. The mip data stays valid, and the shader's existing SSE check naturally uses it for distant rendering. The slot is only reclaimed when the pool is exhausted AND a closer brick needs it.

3. **Air caching** — a cell confirmed as air (source returned None) transitions to the `Air` state and is never re-requested. In the region file, air is written as a sentinel header entry. This prevents the pager from repeatedly regenerating empty cells.

4. **Region format in sandbox, not engine** — the engine is agnostic about persistence. A game could use network streaming, a database, or no caching at all. The `.swr` format is purely a sandbox concern.

## New dependencies

- `crossbeam-channel` 0.5 (engine) — lock-free MPMC channels for the worker pipeline
- `zstd` 0.13 (sandbox) — fast compression for region file payloads
- `bytemuck` (sandbox) — already a workspace dep, added to sandbox for region header I/O

## Verification

- 34 tests pass (28 engine + 6 sandbox, including region round-trip and split_grid_pos)
- Clippy clean, fmt clean
- Bench mode: 7630 bricks stream in via pager over 5 seconds (Terrain Only preset)
- Second launch hits region cache — no worldgen log, identical brick count and performance
