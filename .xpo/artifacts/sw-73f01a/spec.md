# Switchable test scenes

## What

A scene preset system for the sandbox. Each preset defines terrain parameters, object placement, camera start position, and resource budgets. Presets are switchable at runtime via an egui dropdown in the debug panel.

## Why

Right now the sandbox has exactly one hardcoded scene. As engine features land (streaming, lighting, field simulation), we need isolated test cases — terrain-only for profiling worldgen, stress tests for BVH performance, single-brick for debugging shaders, empty for baseline timings. Switching presets at runtime avoids recompilation cycles.

## Presets

| Preset | Terrain | Objects | Pool size | Grid | Camera | Purpose |
|---|---|---|---|---|---|---|
| **Default** | seed 42, 32×12×32 | trees + rocks + pebbles | 32768 | 32×12×32 | (0, 8, 14) pitch -20° | Current scene, regression baseline |
| **Terrain Only** | same | none | 32768 | 32×12×32 | same | Profile worldgen, terrain rendering |
| **Objects Only** | none | 100 trees, 50 rocks on y=0 flat | 8192 | 4×2×4 | (0, 4, 10) pitch -15° | Test instancing, BVH |
| **Stress** | none | 2000+ mixed instances, grid layout | 16384 | 4×2×4 | (0, 20, 40) pitch -25° | BVH + rendering perf ceiling |
| **Single Brick** | one solid brick at origin | none | 256 | 2×2×2 | (0, 1, 3) pitch -10° | Debug rendering, normals, materials |
| **Empty** | none | none | 256 | 2×2×2 | origin | Baseline frame time, pipeline overhead |

## Design

### `scenes.rs` module

```rust
pub enum Preset {
    Default,
    TerrainOnly,
    ObjectsOnly,
    Stress,
    SingleBrick,
    Empty,
}
```

Each variant provides:
- `label() -> &'static str` — display name for the dropdown
- `setup(gpu, pool, index, scene)` — populates brick pool, brick index, and scene
- `camera_start() -> (Vec3, f32, f32)` — position, yaw, pitch
- `grid_dims() -> [u32; 3]` and `world_min() -> Vec3`
- `pool_capacity() -> u32`

Implemented as methods on `Preset` via match arms (not a trait — there's no polymorphism need).

### Scene switching in `RunState`

Add a `load_preset(&mut self, preset: Preset)` method that:
1. Creates new `BrickPool`, `BrickIndex`, `Scene` with the preset's parameters
2. Calls `preset.setup()` to populate them
3. Uploads the scene
4. Constructs a new `Raymarcher` (rebinds all GPU resources)
5. Resets the camera to the preset's start position
6. Stores the current `Preset` for the dropdown

### Egui integration

Add a combo box to the debug panel showing all preset labels. On selection change, call `load_preset()`. The combo box uses `Preset::ALL` (a const array of all variants) for the item list.

### Refactoring `main.rs`

The existing `generate_world()` and `populate_scene()` functions move into `scenes.rs` as the `Default` preset's setup. The `resumed()` handler calls `load_preset(Preset::Default)` instead of inline setup code.

## Flow

1. Create `crates/sandbox/src/scenes.rs` with `Preset` enum and all six setups
2. Move `generate_world()` and `populate_scene()` from `main.rs` into `scenes.rs` as `Preset::Default` setup
3. Add `load_preset()` method to `RunState`
4. Add `current_preset: Preset` field to `RunState`
5. Wire the dropdown into `draw_debug_panel()`
6. Simplify `resumed()` to use `load_preset()`

## Acceptance Criteria

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all --check` clean
- [ ] All six presets load without panic
- [ ] Dropdown switches between presets at runtime
- [ ] Default preset renders identically to the pre-refactor scene
- [ ] Stress preset renders 2000+ instances without crash
- [ ] Single Brick preset shows exactly one solid brick
- [ ] Empty preset shows a clear screen with no geometry
