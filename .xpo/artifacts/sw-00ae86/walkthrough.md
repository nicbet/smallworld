## What was built

Procedural terrain generator that replaces the test sphere+ground scene with a 3D density-based landscape featuring hills with overhangs, underground strata, worm-like caves, and a water table.

## How the pieces fit together

### WorldGenerator (`crates/engine/src/worldgen.rs`)

A deterministic generator seeded by a `u32`. For each voxel at world position (wx, wy, wz):

1. **Terrain density** — `(base_height - wy) / amplitude + (fbm3d(...) - 0.5)`. The noise is centered around 0 so the surface goes both above and below the base height (y=2), creating valleys that can dip below the water table.

2. **Air/water** — density ≤ 0 means above the surface. If below the water table (y=-1), fill with water; otherwise air.

3. **Cave carving** — two intersecting 3D noise fields (`cave_a` and `cave_b`). Both must exceed the threshold (0.48) for a cave to appear. This intersection technique produces narrow, worm-like tunnels rather than the broad smooth voids a single noise field creates. Caves are capped to `density < 1.5` so they don't appear in the deepest rock.

4. **Strata** — material selected by density magnitude (proxy for depth below surface): grass near the surface, then dirt, stone, and dark stone.

### Noise implementation

Self-contained, zero dependencies. Hash-based value noise with smoothstep interpolation, layered into FBM (3 octaves for terrain, 2 for each cave field). The hash uses a Murmur-style multiply-xorshift chain for good distribution. FBM normalizes to [0, 1].

### Performance

The engine crate is compiled with `opt-level = 3` even in dev builds (`profile.dev.package.smallworld-engine`). Without this, the noise math is ~20x slower and worldgen takes 10+ seconds. With optimization, a 32×12×32 grid generates in ~1-2 seconds.

### Viewer changes

Grid expanded to 32×12×32 (51m × 19m × 51m), pool capacity 16384. Camera starts at (0, 8, 14) looking down at -20° to show the full terrain.

## Key decisions

- **Centered noise** (`fbm - 0.5`) for the terrain density. Without this, the surface never dips below the base height and the water table produces no water.

- **Two-field cave intersection** instead of single threshold. `cave_a > t && cave_b > t` creates connected tunnel shapes. A single field produces amorphous blobs that look like terrain erosion rather than caves.

- **opt-level 3 for the engine in dev builds.** Standard Rust pattern for crates with hot math. The viewer stays unoptimized for fast incremental compilation.

## What a future reader should know

- The generator is CPU-only and runs at startup. For larger worlds, generation should move to background threads with brick streaming (epic sw-d6d2c2).

- The water table is a constant y level. More interesting water placement (rivers following terrain gradient, isolated ponds) would need a terrain analysis pass.

- The `Makefile` gained a `screenshot` target: `make screenshot DEST=path DELAY=8` captures just the app window.
