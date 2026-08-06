
# SVO Construction from GPU Noise + Streaming

## What

Build the octree for large worlds directly from the terrain function, in two phases:

1. **Coarse construction** — one-time pass at startup that builds the complete tree *structure* with per-node colors for the entire world, without generating any brick voxel data. The whole world renders immediately at coarse LOD.
2. **Leaf refinement (streaming)** — the existing pager streams full 16³ bricks near the camera (GPU worldgen + disk cache + CPU fallback) and attaches them to leaves. Eviction detaches the brick and the leaf falls back to its averaged color (fixes sw-fcea39).

Target scale: 1 km × 1 km terrain → `world_size = 1638.4 m` (1024 leaf cells/axis, depth-10 tree).

## Why

`preload_all` generates every brick up front — fine at 51.2 m (12k cells), impossible at 1 km (16.7M cells, ~2–3M non-air). The SVO already renders interior nodes as colored cubes; the coarse pass exploits that: distant terrain needs only tree structure + plausible colors, which derive from surface heights — no voxel data, no GPU worldgen, no pool slots.

## Phase 1 — coarse construction (CPU threads)

### Column heightfield + min/max pyramid

- Sample `find_surface_y(wx, wz)` at leaf-column resolution across `std::thread::scope` workers. Effective height = `max(surface, water_level)` so open water registers as occupancy.
- Build a min/max mip pyramid over the effective height grid (one level per tree level), padded ±1 m.

### Recursive build from the pyramid

`coarse_svo::build_coarse` recurses top-down holding parent node indices. Per child node: entirely above footprint max → air (no node); entirely below footprint min → solid coarse leaf (stone strata); crossing → subdivide, with per-column exact leaf classification (grass / water / dirt→stone).

Then one full `update_colors()`; `upload()` after preload (startup only).

### The heightfield lies about caves (review rounds 1–2)

`find_surface_y` scans only terrain density; `sample()` additionally carves caves. Two regressions came from this, fixed during streaming:

1. **Floating coarse boxes** (round 1) — cells painted solid but revealed as air kept their coarse color: opaque slabs over real terrain. Fix: **`Svo::clear_leaf` on every air result** (descends with allocation so cells inside coarse solid nodes subdivide-then-clear), and `recompute_color` zeroes nodes whose children are all invisible.
2. **Flat gray cave walls/floors/ceilings** (round 2) — the air/solid interface of a cavity lies outside every column's heightfield band, so exposed solid cells never streamed. Fix: **6-directional band growth around every discovered air cell** (`extend_bands_around_air`): each neighbor of an air cell becomes a candidate (bands are contiguous, intermediate cells join). The flood is bounded by the cavity surface — solid results stop it. During `preload_radius`, extension requests are capped to the preload radius so a world-spanning cave system cannot stall startup; beyond it cells stay Unknown for demand streaming.

### `Svo` API (as implemented)

- `alloc_child(parent, octant)` — creates/returns the child; **subdividing a coarse solid leaf materializes all 8 children with the parent's color** (prevents air holes).
- `set_color`, `remove_brick` (sw-fcea39), `clear_leaf`, `leaf_info`/`debug_path` (read-only queries).
- `insert_brick`/`clear_leaf` refresh ancestor colors path-locally; interior colors are the unweighted average of direct children.

### Traversal stack (scale-critical, shader)

`trace_svo` restructured from push-all-children (24-entry stack, drops subtrees at depth 10) to cursor-based descent: one frame per level, `MAX_SVO_DEPTH = 16`.

### Node pool

Fixed per preset (Large World 8M nodes / 128 MB; small presets 1M). Measured 1 km need: 3.63M.

## Phase 2 — pager adaptation

- **Streaming set = per-column candidate bands** (heightfield surface + lateral cliff exposure + world borders), grown at runtime around discovered air (see above). Cell states in a `HashMap` of non-Unknown cells only.
- **Eviction** calls `svo.remove_brick` (sw-fcea39).
- **Incremental stats** via transition counters; O(1) per frame.
- **Startup:** GPU worldgen primes via `generate_cells(positions)` — exactly the candidates within the preload radius; `preload_radius` (60 m Large World, everything small presets) blocks; the rest streams by SSE demand.
- **Incremental GPU updates:** path-local color refresh + `upload_dirty` ranged writes.

## Presets

- **Large World**: dims [1024, 16, 1024], `world_size = 1638.4`, terrain only, flythrough, 8 workers.
- Default/TerrainOnly share the two-phase path.

## Known limitations (accepted for this story)

- Detail pop on brick stream-in; tuned in sw-2230ee.
- Distant *unstreamed* cave breaches render the coarse surface estimate until approached.
- Demand box (~30k candidates) vs 32k pool slots near the eviction boundary; churn tuning in sw-2230ee.

## Decisions (user-confirmed 2026-08-06)

1. Coarse pass on CPU threads. 2. Large World preset in this story. 3. Fixed 8M-node pool.

## Acceptance criteria

- [x] Coarse construction of the 1638.4 m world < 5 s — **measured 0.35 s**
- [x] Bricks stream near the camera, SSE-prioritized; no full-grid preload
- [x] Eviction returns leaves to averaged color (sw-fcea39)
- [x] No opaque coarse cells over air AND no exposed solid cell without brick detail after full preload (`preload_leaves_no_floating_boxes`: ground-truth occupancy for every cell, checks both invariants)
- [x] Pager per-frame cost independent of total world size
- [ ] Default preset renders correctly (manual re-check, round 3)
- [ ] Large World flythrough ≥ 30 fps with streaming active (manual check)
