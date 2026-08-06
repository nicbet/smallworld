# Walkthrough: Per-preset camera paths

## What changed

Replaced the generic orbit camera in bench mode with per-preset camera paths defined in `Preset::camera_path(t)`.

## Modified: `crates/sandbox/src/scenes.rs`

Added `camera_path(self, t: f32) -> (Vec3, f32, f32)` to `Preset`. Takes normalized progress `t` (0..1), returns `(position, yaw, pitch)`.

Each path is a parametric curve tuned to the preset's geometry:
- **Default**: r=18 orbit with height oscillating 2–10m (dips to canopy, rises to overview)
- **TerrainOnly**: r=20 orbit with height oscillating 1–7m, faster vertical frequency (3 dips per orbit to sweep through valleys)
- **ObjectsOnly**: variable radius 4–12m, height 1–4m (weaves between objects at eye level)
- **Stress**: radius contracts 15–30m, height oscillates 0–20m (overview then pushes in close)
- **SingleBrick**: r=2.5 tight orbit centered on brick at (0.8, 0.8, 0.8), height 0.5–1.5m
- **Empty**: static camera at (0, 2, 5)

All paths use `atan2` for yaw/pitch pointed at the scene center.

## Modified: `crates/sandbox/src/bench.rs`

Removed all orbit state (`orbit_angle`, `orbit_radius`, `orbit_height`, `orbit_center`), the `Vec3Swizzles` import, and the `advance_orbit` method. Replaced with `advance_camera` which delegates to `self.config.preset.camera_path(t)`. `BenchState::new` no longer takes a `preset` parameter — it reads it from the config.

## Modified: `crates/sandbox/src/main.rs`

Updated call sites: `BenchState::new(config)` (was `new(config, preset)`), `bs.advance_camera(camera)` (was `bs.advance_orbit(dt, camera)`).
