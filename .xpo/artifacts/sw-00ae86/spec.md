## What

Replace the test sphere+ground scene with a procedural terrain generator: 3D density-based surface with overhangs, underground strata, caves, and a water table. Deterministic from seed, generates bricks on demand.

## Why

M1 exit criterion: "free-fly a generated sparse world with sun shadows." The test sphere proves the toolchain; this proves the world.

## Acceptance Criteria

- 3D density function produces a terrain with hills, overhangs, and caves
- Underground has depth-based strata (grass → dirt → stone → dark stone)
- Water table fills depressions as a still-water material
- Deterministic from seed
- World large enough to fly around (~100m × 25m × 100m)
- Interactive frame rates maintained

## Flow

### 1. New module — `crates/engine/src/worldgen.rs`

`WorldGenerator` with:
- `new(seed: u32)` — set terrain parameters
- `generate_brick(grid_pos, world_min) -> Option<BrickData>` — returns voxels + palette if brick has content
- Self-contained noise: hash-based value noise + FBM (no external dependency)

**Terrain model:**
- 3D density: `density = (base_height - wy) / amplitude + fbm3d(wx, wy, wz)`
- density > 0 → solid, density ≤ 0 → air (or water if below water table)
- Cave carving: separate 3D FBM, threshold-based subtraction
- Material from density magnitude: near-surface → grass/dirt, deep → stone/dark stone

**Materials:**
| Index | Name | RGBA |
|-------|------|------|
| 0 | Air | — |
| 1 | Grass | (76, 153, 0) |
| 2 | Dirt | (139, 90, 43) |
| 3 | Stone | (128, 128, 128) |
| 4 | Dark stone | (80, 80, 90) |
| 5 | Water | (30, 100, 180) |

### 2. Viewer changes — `main.rs`

- Grid: 32×16×32 bricks (51m × 26m × 51m), pool capacity 16384
- Replace `generate_test_world()` with generator loop
- Camera positioned above terrain

## Decisions

**D1: Inline hash-based value noise, not the `noise` crate.** Zero dependencies, ~50 lines, fast enough for CPU generation. FBM layering gives multi-scale features.

**D2: 3D density function, not a 2D heightmap.** The issue explicitly requires this. Produces overhangs and cave openings to the surface. Material selection uses density magnitude as a proxy for depth below surface.

**D3: Water as opaque voxels for now.** DESIGN.md D9 says "fluid-as-material now, sim state later." Water voxels render as opaque blue; transmission rendering is a future issue.
