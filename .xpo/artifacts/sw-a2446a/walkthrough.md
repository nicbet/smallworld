
# Walkthrough: winit window + wgpu device/surface init with egui debug overlay

## What was built

A live GPU application replacing the previous print-and-exit smoke test. The viewer now
opens a resizable window backed by wgpu on Metal (macOS) or Vulkan (Windows/Linux), renders
a cleared background, and draws an egui debug overlay showing adapter info, window
dimensions, camera state, and frame timing. A free-fly camera responds to WASD + mouse-look.

## Dependency stack

All GPU dependencies are pinned as workspace-level entries in the root `Cargo.toml`:

| Crate | Version | Role |
|-------|---------|------|
| wgpu | 30 | GPU abstraction (Metal/Vulkan/DX12) |
| winit | 0.30 | Window + input events |
| egui | 0.36 | Immediate-mode debug UI |
| egui-wgpu | 0.36 | egui → wgpu render backend |
| egui-winit | 0.36 | egui ← winit input bridge |
| glam | 0.33 | Vec3/Mat4 math for camera |
| pollster | 0.4 | Block on async adapter/device requests |
| env_logger | 0.11 | `RUST_LOG`-driven logging |

`rust-version` was bumped from 1.94 → 1.97 because egui 0.36 requires it.

The engine crate re-exports `wgpu` (`pub use wgpu;` in `lib.rs`) so that the viewer
and all future crates share a single version — no direct wgpu dependency in the viewer.

## How the pieces fit together

### Engine crate (`crates/engine/`)

**`gpu.rs` — `GpuContext`**: Holds `Instance`, `Adapter`, `Device`, `Queue`. Two
constructors:

- `new(instance, &surface)` — production path: requests a high-performance adapter
  compatible with the given surface, then creates the device.
- `headless(instance)` — CI path: requests an adapter without a surface, used by the
  `--info` flag.

`configure_surface()` picks an sRGB format from the surface capabilities and applies
AutoVsync present mode. Returns the `SurfaceConfiguration` so the caller can read back
width/height/format.

`create_instance()` builds a `wgpu::Instance` with all backends enabled.

**`camera.rs` — `FreeCamera`**: Pure math, no input handling. Stores position, yaw, pitch,
FOV, aspect, near/far. Exposes `forward()`, `right()`, `view_matrix()`,
`projection_matrix()`, `translate(local_delta)`, `rotate(yaw_delta, pitch_delta)`.
Pitch is clamped to ±89°. Three unit tests cover the invariants.

### Viewer crate (`crates/viewer/src/main.rs`)

**`main()`**: Checks for `--info` (headless adapter probe → print and exit). Otherwise
creates a winit `EventLoop` and runs the `App` via `ApplicationHandler`.

**`App` / `RunState`**: `App` holds `Option<RunState>`. `RunState` is initialized in
`resumed()` — creates the window, wgpu surface, `GpuContext`, egui context + state +
renderer, and camera. This deferred init is required by winit 0.30's
`ApplicationHandler` model where the window can only be created after the event loop is
running.

**Event loop flow**:

1. `resumed` → create window, surface, GPU context, egui, camera.
2. `about_to_wait` → `window.request_redraw()` (continuous rendering).
3. `window_event(RedrawRequested)`:
   - Compute frame delta time, apply camera movement from held keys.
   - Run egui (`run_ui`) to build the debug panel.
   - Acquire surface texture (skip frame on Timeout/Occluded, reconfigure on Lost/Outdated).
   - Upload egui texture deltas (drain, not iterate — epaint 0.36 asserts on drop if
     deltas remain unconsumed).
   - If a surface frame was acquired: begin render pass, clear to dark grey, render egui,
     present.
4. `window_event(KeyboardInput)` → WASD/Space/Q/E/Ctrl/Shift/Escape mapped to `InputState`.
5. `device_event(MouseMotion)` → right-click-drag rotates camera.
6. `window_event(Resized)` → reconfigure surface, update camera aspect.

**Debug overlay** (`draw_debug_panel`): A collapsible egui window showing adapter name +
backend, driver, window dimensions, camera position/yaw/pitch, and frame time + FPS.

### CI and Makefile

The smoke test changed from `cargo run -p smallworld-viewer` (which would try to open a
window) to `cargo run -p smallworld-viewer -- --info` (headless adapter probe). The
Makefile gained a `smoke` target and `ci` now calls `smoke` instead of `run`.

## Key decisions and rationale

1. **wgpu 30 + egui 0.36 over wgpu 29 + egui 0.35**: egui-wgpu 0.35 depends on wgpu 29,
   which has a substantially different API from wgpu 30 (no `CurrentSurfaceTexture` enum,
   different `InstanceDescriptor`, etc.). Fighting the mismatch was not productive; bumping
   rust-version to 1.97 was the simpler path.

2. **`drain()` for texture deltas**: epaint 0.36 added a `debug_assert!` in
   `TexturesDelta::drop` that panics if deltas exist but weren't consumed. Iterating by
   reference (`&`) doesn't clear the collection — `drain()` does.

3. **`forget_lifetime()` on render pass**: wgpu 30 changed `begin_render_pass` to return
   `RenderPass<'encoder>`, but egui-wgpu's `render()` takes `&mut RenderPass<'static>`.
   The egui-wgpu crate documents `forget_lifetime()` as the intended bridge.

4. **egui runs every frame regardless of surface availability**: If egui only runs when a
   surface frame is acquired, the font atlas delta accumulates unconsumed on occluded
   frames and triggers the drop assertion. Running egui + uploading textures every frame
   (even without presenting) avoids this.

## Files changed

| File | Change |
|------|--------|
| `Cargo.toml` | Workspace deps for wgpu/winit/egui/glam/pollster/env_logger; rust-version → 1.97 |
| `crates/engine/Cargo.toml` | wgpu, glam, log as workspace deps |
| `crates/engine/src/lib.rs` | Added `gpu`, `camera` modules; `pub use wgpu` |
| `crates/engine/src/gpu.rs` | **New** — `GpuContext` struct |
| `crates/engine/src/camera.rs` | **New** — `FreeCamera` struct + tests |
| `crates/viewer/Cargo.toml` | egui, egui-wgpu, egui-winit, winit, glam, pollster, env_logger, log |
| `crates/viewer/src/main.rs` | **Rewritten** — windowed GPU app with egui overlay |
| `.github/workflows/ci.yml` | Smoke test uses `--info` flag |
| `Makefile` | Added `smoke` target; `ci` calls `smoke` instead of `run` |
