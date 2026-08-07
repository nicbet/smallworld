## What was built

The `Engine` struct — the top-level entry point for any game using smallworld. It owns all GPU plumbing (GpuContext, DisplaySurface), the World→GPU extraction boundary, and the application event loop. `World` became pure CPU data with no GPU dependency. The old sandbox was renamed to `sandbox_old` as a visual reference, and a new sandbox was built from scratch using the Engine API.

## Why

The old sandbox threaded `GpuContext`, `Surface`, `SurfaceConfiguration`, and `&device` through every call — leaking wgpu internals into game-facing code. `World` stored a `Device` clone, coupling entities to GPU. In Unity, Unreal, and Godot the game mutates the world and the engine handles everything else. Our API now works the same way.

## How the pieces fit together

### New: `crates/engine/src/engine.rs`

**`EngineConfig`** — intent-based initialization: title, `WindowMode` (Windowed/Fullscreen), vsync.

**`Engine`** — owns:
- `Option<Arc<Window>>` — None for headless
- `GpuContext` — device, queue, adapter
- `Option<DisplaySurface>` — bundles wgpu Surface + SurfaceConfiguration as one concept
- `WorldGpuData` — cached extraction result, updated automatically

**`Engine::run(config, world, update_fn)`** — the main entry point. Owns the winit EventLoop, creates window + GPU, runs the frame loop. Handles resize, close, escape, surface acquire/present internally. Calls the game's update function each frame with `(&mut Engine, &mut World, f32)`.

The `AppRunner` struct implements winit's `ApplicationHandler` internally — the game never sees it. Frame lifecycle:
1. Engine captures frame timing
2. Calls `update(engine, world, dt)` — game logic
3. Calls `begin_frame(world)` — extracts if dirty, acquires surface
4. Calls `present(frame)`

**`Engine::headless()`** — no window, no surface. For CI and testing.

**`Engine::begin_frame(&mut World)`** — checks `world.is_dirty()`, extracts to `WorldGpuData` if needed, clears dirty flag, acquires surface texture. Returns `Option<FrameContext>`.

**`FrameContext`** — wraps `SurfaceTexture`, exposes `view()` for render pass attachment. Keeps wgpu types out of game code.

### Changed: `crates/engine/src/world.rs`

`World` is now pure CPU data:
- `World::new()` takes no arguments (was `&wgpu::Device`)
- `device` field removed
- `gpu_data` field removed
- `extract()` method removed — extraction moved to `WorldGpuData::extract(&Device, &World)`, called by Engine
- `is_dirty()` and `clear_dirty()` are `pub(crate)` — only Engine calls them
- `Default` trait implemented

### New: `crates/sandbox/`

Built from scratch. The entire sandbox is:
```rust
fn main() {
    env_logger::init();
    Engine::run(EngineConfig::default(), World::new(), update);
}

fn update(_engine: &mut Engine, _world: &mut World, _dt: f32) {
    // Game logic goes here.
}
```

13 lines. Engine handles all the ceremony. The update function is a plain `fn`, not a closure — scales cleanly as the game grows (own module, own file).

### Renamed: `crates/sandbox_old/`

The old sandbox preserved as visual reference. Not in the workspace — just for eyeball comparison.

## Key decisions

- **`Engine::run` absorbs the event loop** — games never implement `ApplicationHandler`. The update closure/function is the only game-provided code. This follows the pattern where 80% of the old sandbox was winit boilerplate.

- **World is GPU-free** — no `use wgpu` in world.rs. The entity layer is pure Rust data structures. Engine bridges the gap via `WorldGpuData::extract()`.

- **Extraction is engine-internal** — the game never sees `is_dirty()`, `extract()`, or `WorldGpuData` construction. `begin_frame` handles it automatically, matching Unity/Unreal/Godot where the engine handles the data bridge.

- **Update function, not closure** — `Engine::run` takes `impl FnMut(...)` so both work, but the sandbox uses a named `fn update` for clarity. Scales better than nesting game logic inside a closure.

- **New sandbox from scratch** — instead of retrofitting the old sandbox at each story (repeated churn on the same 1000-line file), we built clean and grow from here.

- **`DisplaySurface` bundles Surface + config** — one concept internally, game never sees either.

## Non-obvious details

- `Engine::new()` is still `pub` for advanced use cases where the game wants to manage its own event loop. `Engine::run` is the recommended path.

- The `AppRunner` holds `Option<World>` during the gap between `run()` and `resumed()` (winit creates the window asynchronously). It moves into `AppRunnerState` on first resume.

- `pollster` and `winit` are now engine dependencies (moved from sandbox-only).
