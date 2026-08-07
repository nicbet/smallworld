## What was built

Frame-captured input snapshot system and the `App` trait for game state management. Games implement `App` on their state struct; the engine calls `update(&mut self, ...)` each frame with a stable input snapshot.

## Why

The update function had no way to read user input, and game state had no clean home — it was captured in closures. Every major engine (Unity, Unreal, Godot, ggez) converges on the same two patterns: snapshot-based input polling and a game state struct with an update method.

## How the pieces fit together

### New: `crates/engine/src/input.rs`

**`Input`** — frame-captured snapshot with three tiers of key/button state:
- `key_held` / `mouse_held` — level-triggered, true while down
- `key_pressed` / `mouse_pressed` — edge-triggered, true only on the down frame
- `key_released` / `mouse_released` — edge-triggered, true only on the up frame
- `mouse_position` — cursor in window-logical pixels (from `CursorMoved`)
- `mouse_delta` — raw accumulated motion (from `DeviceEvent::MouseMotion`, unclipped)

**`ControllerState`** — placeholder for generic controllers (gamepads, joysticks, flight sticks). Numbered axes (`Vec<f32>`) and numbered buttons (`HashSet<u32>`), not named. A gamepad maps to axes 0-5 and buttons 0-15; a flight stick maps differently. The game or an action mapping layer decides what each index means. `Input::controller(index)` returns `Option<&ControllerState>` — `None` until controller support is wired up.

**Re-exports** — `pub use winit::keyboard::KeyCode` and `pub use winit::event::MouseButton` so games import from `smallworld_engine::input`, never from winit directly.

**Frame lifecycle:**
1. Between frames: Engine accumulates winit `KeyboardInput`, `MouseInput`, `CursorMoved`, `MouseMotion` events into the `Input` struct
2. After update: `Input::begin_frame()` clears edge-triggered sets and resets mouse delta
3. During update: game reads `engine.input()`, stable for the entire call

### Changed: `crates/engine/src/engine.rs`

**`App` trait** — replaces the `FnMut` closure. Games implement this on their state struct:
```rust
pub trait App {
    fn update(&mut self, engine: &mut Engine, world: &mut World, dt: f32);
}
```
Extensible — `fn init()`, `fn on_resize()`, `fn shutdown()` can be added as default methods later without breaking existing games.

**`Engine::run`** — signature changed from `impl FnMut(...)` to `impl App + 'static`. Internally, `AppRunner` stores `Box<dyn App>` and calls `self.app.update(...)` in `RedrawRequested`.

**`Engine.input`** — field added, `engine.input()` accessor returns `&Input`. Event accumulation wired into `AppRunner::window_event` (keyboard, mouse buttons, cursor) and `device_event` (raw mouse motion).

### New: `crates/sandbox/src/camera_rig.rs`

`CameraRig` wrapping `FreeCamera` with an `update(&mut self, &Input, f32)` method. WASD movement, shift-sprint, right-click mouse look. Keeps camera logic out of main.

### Changed: `crates/sandbox/src/main.rs`

Now follows the standard engine game pattern:
```rust
struct Game { camera: CameraRig }
impl App for Game {
    fn update(&mut self, engine: &mut Engine, _world: &mut World, dt: f32) {
        self.camera.update(engine.input(), dt);
    }
}
fn main() {
    Engine::run(EngineConfig::default(), World::new(), Game { camera: CameraRig::new() });
}
```

## Key decisions

- **Snapshot, not events** — input is captured once at frame start and stable during update. This is what Unity, Unreal, and Godot all do. Async input would cause mid-update state changes and race conditions.

- **`App` trait, not closure** — game state lives as fields on a struct, update is a method. Matches the pattern every major engine converges on (MonoBehaviour, AActor, EventHandler). Extensible with lifecycle methods.

- **Generic controllers, not gamepads** — `ControllerState` uses numbered axes and buttons. A gamepad, joystick, or steering wheel all fit the same model. The game decides what axis 0 means.

- **Raw mouse delta from `DeviceEvent::MouseMotion`** — not `CursorMoved`. `MouseMotion` gives unclipped deltas that work at window edges (essential for camera look). `CursorMoved` gives absolute position (for UI cursor).

- **Escape stays in engine** — hardcoded quit-on-escape is infrastructure, not game logic. Games that want to override it can handle it later when we add an `fn on_input` lifecycle method.
