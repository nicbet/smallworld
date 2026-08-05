
# Spec: winit window + wgpu device/surface init with egui debug overlay

## What

Stand up the runtime graphics stack: a resizable winit window, wgpu device/surface with
swapchain management, a free-fly camera with WASD + mouse-look, and an egui debug overlay
showing adapter info, frame dimensions, and camera state. The viewer becomes a live GPU
application instead of a print-and-exit smoke test.

## Why

This is the first story in M0 (sw-4df655) that puts pixels on screen. Everything downstream —
the compute raymarcher (sw-ce3a9f), GPU timestamp queries (sw-c96adf), and every milestone
after — needs a window, a device, and a surface to render into. The egui overlay is the
debug cockpit that DESIGN.md insists on from day one ("measurement tooling is non-negotiable").
The free-fly camera gives the raymarcher a viewpoint to trace from.

## Acceptance Criteria

1. `cargo run -p smallworld-viewer` opens a resizable window with a cleared background.
2. The wgpu adapter selects Metal on macOS. Adapter name and backend are visible in the
   egui overlay (verifies DESIGN.md risk: "validate on Metal early").
3. Window resize is handled without crashes — surface is reconfigured on each resize.
4. Free-fly camera: WASD translates, mouse-look (right-click drag or cursor-captured) rotates,
   Q/E for roll or vertical, Shift for speed boost. Camera position + orientation shown in overlay.
5. egui overlay displays: adapter name, backend API, window dimensions, camera position/yaw/pitch,
   FPS (frame delta).
6. Escape key exits the application.
7. `cargo run -p smallworld-viewer -- --info` prints adapter info and exits without opening a
   window (CI smoke test replacement).
8. `make ci` passes — lint, test, build, and the updated smoke-test command all green.
9. All three CI platforms (macOS/Metal, Linux/Vulkan, Windows/Vulkan+DX12) build and pass
   clippy + tests.

## Flow

### 1. Add workspace dependencies (root `Cargo.toml`)

Pin versions in `[workspace.dependencies]`:
- `wgpu` — GPU abstraction
- `winit` — window + input events
- `egui` — immediate-mode UI
- `egui-wgpu` — egui wgpu rendering backend
- `egui-winit` — egui winit input integration
- `glam` — vec3/mat4/quat math for the camera
- `log` + `env_logger` — structured logging

### 2. Engine crate: GPU context module (`crates/engine/src/gpu.rs`)

New module exposing:
- `GpuContext` struct: holds `wgpu::Device`, `wgpu::Queue`, `wgpu::Adapter`,
  `wgpu::Instance`.
- `GpuContext::new(instance, surface) -> Self` — requests adapter (prefer high-performance,
  compatible with given surface), requests device with default limits/features.
- `AdapterInfo` accessor for the overlay to read adapter name, backend, driver.
- The engine owns GPU state because the raymarcher (next story) lives here.

Re-export `wgpu` from the engine crate so the viewer doesn't depend on wgpu directly —
single version, single source of truth.

### 3. Engine crate: Camera module (`crates/engine/src/camera.rs`)

- `FreeCamera` struct: position (`Vec3`), yaw, pitch (radians), aspect ratio, fov, near/far.
- `FreeCamera::view_matrix() -> Mat4`, `projection_matrix() -> Mat4`.
- `FreeCamera::translate(delta: Vec3)` — move in camera-local space.
- `FreeCamera::rotate(yaw_delta, pitch_delta)` — clamp pitch to ±89°.
- No input handling here — pure math. The viewer maps keys/mouse to these methods.

### 4. Viewer crate: rewrite `main.rs`

Replace the print-and-exit main with:
- Parse `--info` flag. If present: create wgpu instance, request adapter, print info, exit.
- Otherwise: create winit event loop + window, create wgpu surface from window, init
  `GpuContext`, init egui (via `egui-winit` + `egui-wgpu`), create `FreeCamera`.
- Event loop (`ApplicationHandler` trait — winit 0.30+ API):
  - `Resumed`: create surface, configure swapchain.
  - `WindowEvent::Resized`: reconfigure surface.
  - `WindowEvent::RedrawRequested`: acquire frame → clear to dark grey → render egui → present.
  - `WindowEvent::KeyboardInput`: WASD/QE/Shift/Escape.
  - `WindowEvent::MouseInput` + `CursorMoved`: right-click-drag for mouse look.
  - `WindowEvent::CloseRequested`: exit.
  - `AboutToWait`: request redraw (continuous rendering).
- Camera movement applied per frame using frame delta time.

### 5. egui debug overlay

A collapsible egui window ("Debug") in the top-left corner showing:
- **Adapter**: name, backend (e.g. "Apple M1 Pro — Metal")
- **Window**: width × height
- **Camera**: position (x, y, z), yaw°, pitch°
- **Frame**: delta ms, FPS

### 6. Update CI and Makefile

- Change the smoke-test command from `cargo run -p smallworld-viewer` to
  `cargo run -p smallworld-viewer -- --info` in both `ci.yml` and the `Makefile`.
- The `--info` path exercises adapter creation without needing a display server.

## Decisions

| # | Choice | Alternative | Rationale |
|---|--------|-------------|-----------|
| 1 | Re-export `wgpu` from engine; viewer depends only on engine | Viewer depends on wgpu directly | Single version pin, engine owns GPU state per DESIGN.md |
| 2 | Camera struct in engine, input mapping in viewer | Camera entirely in viewer | The raymarcher (next story) needs the view/projection matrices from engine |
| 3 | `--info` flag for headless CI smoke test | `CI` env-var check / xvfb-run | Explicit, self-documenting, works on all three CI platforms without display hacks |
| 4 | Right-click-drag for mouse look (not cursor capture) | Permanent cursor capture | Less disruptive during development; cursor capture can be added later as a toggle |
| 5 | winit 0.30+ `ApplicationHandler` trait API | Older closure-based `run()` | The modern API; 0.29 is deprecated |
| 6 | `glam` for math | `nalgebra`, `ultraviolet` | Lightest, most wgpu-ecosystem-aligned, `bytemuck`-friendly for future GPU uploads |

## Edge Cases

- **Minimized window (zero-size surface)**: skip `acquire_frame` + present when either
  dimension is 0. (LOW — handle silently)
- **Adapter not found**: log error and exit with a clear message. (LOW)
- **Multiple monitors / HiDPI**: use winit's `scale_factor` for surface config; egui-winit
  handles DPI scaling automatically. (LOW)

## Assumptions

- The cleared background is sufficient for this story; the raymarcher (sw-ce3a9f) will
  replace it with a compute pass.
- No gamepad/controller support needed yet.
- Frame timing uses `Instant::now()` delta — no GPU timing yet (that's sw-c96adf).

## Open Questions

1. **Camera speed / FOV defaults** — I'll use 5 m/s base speed (20 m/s with shift), 60° vertical
   FOV. These are adjustable constants. Acceptable?
2. **egui panel style** — transparent background with light text, or opaque panel? I'll default
   to egui's standard semi-transparent panel. Acceptable?
