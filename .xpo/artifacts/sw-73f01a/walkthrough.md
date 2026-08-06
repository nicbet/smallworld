# Walkthrough: Switchable test scenes

## What changed

Added a scene preset system to the sandbox with six switchable presets, selectable at runtime via an egui combo box in the debug panel.

## New file: `crates/sandbox/src/scenes.rs`

`Preset` enum with variants: `Default`, `TerrainOnly`, `ObjectsOnly`, `Stress`, `SingleBrick`, `Empty`. Each variant provides:
- `grid_dims()` / `world_min()` / `pool_capacity()` — resource sizing
- `camera_start()` — (position, yaw, pitch)
- `setup()` — populates BrickPool, BrickIndex, and Scene

The existing `generate_world()` and `populate_scene()` functions from `main.rs` became `generate_terrain()` and `populate_default_objects()` inside `scenes.rs`, called by `Preset::Default`.

## Modified: `crates/sandbox/src/main.rs`

- Added `current_preset: Preset` field to `RunState`
- Added `RunState::load_preset()` — creates fresh BrickPool, BrickIndex, Scene, rebuilds Raymarcher, resets camera
- `resumed()` delegates initial setup to `Preset::Default` instead of inline code
- Egui `ComboBox` wired into the debug panel; selection change triggers `load_preset()`

## Bug fix: `crates/engine/src/brick_index.rs`

`BrickIndex::new` previously created the GPU buffer with `mapped_at_creation: false`, leaving it uninitialized. Presets without terrain (ObjectsOnly, Stress, Empty) never called `upload()`, so the shader read garbage from the grid map and rendered phantom bricks (the "triangle hedges"). Fixed by using `mapped_at_creation: true` and filling with `u32::MAX` (EMPTY) immediately.

## Bug fix: `crates/engine/src/raymarcher.rs`

The dummy buffer (used when scene has no instances/BVH) was 16 bytes, but `ObjectInstance` in the shader is 176 bytes. wgpu validates minimum buffer size against the struct stride. Bumped to 256 bytes.

## Preset details

| Preset | Terrain | Objects | Pool | Camera |
|---|---|---|---|---|
| Default | 32×12×32, seed 42 | trees + rocks + pebbles | 32768 | (0,8,14) -20° |
| Terrain Only | same | none | 32768 | same |
| Objects Only | none | trees + rocks on y=0 | 8192 | (0,4,10) -15° |
| Stress | none | 484 mixed instances, grid | 16384 | (0,20,40) -25° |
| Single Brick | 1 debug brick at origin | none | 256 | (0,1,3) -10° |
| Empty | none | none | 256 | (0,2,5) |

## Key decisions

- Scene switching reconstructs the full Raymarcher rather than adding a `rebind()` method — switching is a dev-time operation, the pipeline creation cost is negligible.
- SingleBrick uses a 4-material debug palette (red=interior, green=face, blue=edge, yellow=corner) to validate normal/material rendering at brick boundaries.
- `_device` parameter kept in `setup()` signature for forward compatibility with presets that create GPU resources directly.
