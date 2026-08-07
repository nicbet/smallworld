## What

Introduce an `Engine` struct that owns all GPU plumbing, the display surface, and the World-to-GPU extraction boundary. `World` becomes pure CPU data with no GPU dependency. The game mutates the world; the engine handles everything else — extraction, surface management, frame lifecycle. No wgpu types in game-facing code.

## Why

Currently the sandbox manually creates and threads `GpuContext`, `Surface`, `SurfaceConfiguration`, and `&device` through every call. `World` stores a `Device` clone for extraction — coupling the entity layer to GPU. In Unity, Unreal, and Godot the game never thinks about extraction — it mutates the world and the engine handles getting data to the GPU as part of the frame. Our API should work the same way.

## Acceptance Criteria

- [ ] `Engine` struct owning GPU context + display surface + cached `WorldGpuData`
- [ ] `EngineConfig` with `WindowMode`, title, vsync
- [ ] `Engine::new()` takes config + `ActiveEventLoop`
- [ ] `Engine::headless()` for CI/testing
- [ ] `Engine::resize()` handles surface reconfiguration
- [ ] `Engine::begin_frame(&mut World)` — extracts if dirty, acquires surface
- [ ] `Engine::present()` wraps presentation
- [ ] `Engine::gpu_data()` returns cached `&WorldGpuData` for renderer
- [ ] `World::new()` takes no arguments — pure CPU data, no wgpu dependency
- [ ] Sandbox migrated — game code never sees `is_dirty`, `extract`, or `WorldGpuData` construction
- [ ] All existing scenes render identically
- [ ] `make test` and `make lint` pass

## Design

### WindowMode

```rust
pub enum WindowMode {
    Windowed { width: u32, height: u32 },
    Fullscreen,
}
```

### EngineConfig

```rust
pub struct EngineConfig {
    pub title: String,
    pub window_mode: WindowMode,
    pub vsync: bool,
}
```

Default: `"smallworld"`, `Windowed { 1280, 720 }`, vsync on.

### DisplaySurface (private)

```rust
struct DisplaySurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}
```

Bundles wgpu surface + config. Private inside Engine. `None` in headless mode.

### Engine

```rust
pub struct Engine {
    window: Option<Arc<Window>>,
    gpu: GpuContext,
    display: Option<DisplaySurface>,
    gpu_data: WorldGpuData,
}
```

Engine caches `WorldGpuData` internally. Updated during `begin_frame` when the world is dirty.

#### Construction

```rust
impl Engine {
    pub fn new(config: EngineConfig, event_loop: &ActiveEventLoop) -> Self;
    pub fn headless() -> Self;
}
```

#### Frame lifecycle

```rust
impl Engine {
    pub fn begin_frame(&mut self, world: &mut World) -> Option<FrameContext>;
    pub fn present(&self, frame: FrameContext);
}
```

`begin_frame`:
1. Checks `world.is_dirty()` — if true, extracts and caches new `WorldGpuData`, clears dirty flag
2. Acquires surface texture (handles Lost/Outdated/Timeout internally)
3. Returns `Some(FrameContext)` with the surface texture, or `None` to skip

`present`: presents the frame.

`FrameContext` is a thin wrapper around `SurfaceTexture` — keeps the wgpu type out of game code's vocabulary:

```rust
pub struct FrameContext {
    surface_texture: wgpu::SurfaceTexture,
}

impl FrameContext {
    pub fn view(&self) -> wgpu::TextureView;
}
```

#### GPU data access

```rust
impl Engine {
    pub fn gpu_data(&self) -> &WorldGpuData;
}
```

Returns the cached extraction result. Always valid — empty `WorldGpuData` for a fresh engine, updated during `begin_frame`. The renderer calls this, not the game.

#### Runtime mutation

```rust
impl Engine {
    pub fn resize(&mut self, width: u32, height: u32);
    pub fn set_vsync(&mut self, enabled: bool);
}
```

#### Accessors

```rust
impl Engine {
    pub fn device(&self) -> &wgpu::Device;
    pub fn queue(&self) -> &wgpu::Queue;
    pub fn gpu(&self) -> &GpuContext;
    pub fn surface_format(&self) -> wgpu::TextureFormat;
    pub fn surface_size(&self) -> (u32, u32);
    pub fn window(&self) -> Option<&Window>;
    pub fn adapter_info(&self) -> wgpu::AdapterInfo;
    pub fn supports_timestamps(&self) -> bool;
}
```

Transitional — subsystems still need `&GpuContext`. Narrows as more moves behind Engine.

### World changes

```rust
pub struct World {
    models: Vec<VoxelModel>,
    instances: SlotMap<InstanceKey, VoxelInstance>,
    dirty: bool,
}

impl World {
    pub fn new() -> Self;             // no args
    pub(crate) fn is_dirty(&self) -> bool;
    pub(crate) fn clear_dirty(&mut self);
    // add_model, add_instance, get, get_mut, remove_instance — unchanged
}
```

`World` drops the `device` field and `gpu_data` cache. `extract()` method removed. `is_dirty()` and `clear_dirty()` are `pub(crate)` — only Engine calls them, the game never sees them.

### Game loop pattern

```rust
// Init
let engine = Engine::new(config, event_loop);
let mut world = World::new();
world.add_instance(Instance { .. });

// Frame
if let Some(frame) = engine.begin_frame(&mut world) {
    let view = frame.view();
    raymarcher.compute_pass(engine.gpu(), &mut encoder, &camera, &svo, engine.gpu_data(), flags, sse, ts);
    raymarcher.blit_pass(&mut encoder, &view, blit_ts);
    engine.present(frame);
}
```

The game mutates the world, calls `begin_frame`, renders, presents. No `is_dirty`, no `extract`, no `WorldGpuData` construction. Those are engine internals.

## Flow

1. **Create `engine.rs`** — `Engine`, `EngineConfig`, `WindowMode`, `DisplaySurface`, `FrameContext`, construction, frame lifecycle, extraction (moved from World), accessors.

2. **Simplify `world.rs`** — remove `device` field, `gpu_data` field, `extract()` method. `World::new()` takes no args. Add `pub(crate) is_dirty()` and `clear_dirty()`.

3. **Move extraction logic** — `Engine::extract_world(&mut self, world: &mut World)` as a private method called from `begin_frame`. Same BVH build + GPU pack logic, uses `self.gpu.device`.

4. **Migrate sandbox `main.rs`** — Replace `window + gpu + surface + surface_config` with `engine: Engine`. Replace `World::new(&device)` with `World::new()`. Replace manual surface acquire/present with `begin_frame`/`present`. Replace `world.extract()` with `engine.gpu_data()`.

5. **Migrate sandbox resize** — `engine.resize(w, h)`.

6. **Migrate `--info` path** — `Engine::headless()`.

7. **Migrate tests in `coarse_svo.rs`** — headless tests use `Engine::headless()`.

8. **Test** — `make test`, `make lint`, visual check.

## Decisions

- **Extraction is engine-internal** — the game never calls extract. `begin_frame` checks dirty and extracts automatically. This matches Unity/Unreal/Godot where the engine handles the data bridge.

- **`is_dirty()` and `clear_dirty()` are `pub(crate)`** — only Engine accesses them. The game never thinks about dirty tracking.

- **`FrameContext` wraps `SurfaceTexture`** — keeps wgpu types out of the game's vocabulary. The game gets a `view()` for render pass attachment. Minimal wrapper, no overhead.

- **`WorldGpuData` cached on Engine** — Engine owns the GPU data, updated lazily. `gpu_data()` is always valid (empty for fresh engine, populated after first `begin_frame`).

- **World has zero wgpu dependency** — no `use wgpu` in world.rs. The entity layer is pure Rust data structures.

- **Engine does NOT own renderer** — raymarcher, egui, timestamps, camera remain sandbox-owned. Renderer ownership comes with sw-97e655.

- **`GpuContext` stays as a type** — Engine wraps it, exposes `gpu()`. Transitional.

## Edge Cases

- **Surface lost/outdated** — `begin_frame()` reconfigures internally, returns None for that frame.
- **Zero-size window** — `resize()` clamps to (1, 1).
- **Headless engine** — no display, `begin_frame()` still extracts if dirty but returns None (no surface to acquire). `gpu_data()` still valid for headless rendering to texture.
- **First frame** — `gpu_data()` returns empty `WorldGpuData` until first `begin_frame` with a non-empty world. Raymarcher handles this via dummy buffer fallbacks.
- **No fullscreen yet** — variant exists, initial implementation handles `Windowed` only.
