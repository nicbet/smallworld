## What

An `Input` struct that captures keyboard and mouse state each frame. The game polls it during update — `engine.input().key_held(W)`, `engine.input().mouse_delta()`. Engine accumulates winit events between frames and snapshots them before calling update.

## Why

The update function has no way to read user input. Without it the sandbox can't move a camera, respond to clicks, or do anything interactive.

## Acceptance Criteria

- [ ] `Input` struct with keyboard held/pressed/released, mouse position, mouse delta, mouse buttons
- [ ] `engine.input()` returns `&Input`
- [ ] Engine accumulates winit events and snapshots before each update call
- [ ] `key_pressed` = true only on the frame the key went down; `key_held` = true while down; `key_released` = true only on the frame the key went up
- [ ] Sandbox uses input to move a camera with WASD + mouse look
- [ ] `make test` and `make lint` pass

## Design

### Input

```rust
pub struct Input {
    keys_held: HashSet<KeyCode>,
    keys_pressed: HashSet<KeyCode>,
    keys_released: HashSet<KeyCode>,
    mouse_buttons_held: HashSet<MouseButton>,
    mouse_buttons_pressed: HashSet<MouseButton>,
    mouse_buttons_released: HashSet<MouseButton>,
    mouse_position: [f32; 2],
    mouse_delta: [f32; 2],
}
```

Query methods:
```rust
impl Input {
    pub fn key_held(&self, key: KeyCode) -> bool;
    pub fn key_pressed(&self, key: KeyCode) -> bool;
    pub fn key_released(&self, key: KeyCode) -> bool;
    pub fn mouse_held(&self, button: MouseButton) -> bool;
    pub fn mouse_pressed(&self, button: MouseButton) -> bool;
    pub fn mouse_released(&self, button: MouseButton) -> bool;
    pub fn mouse_position(&self) -> [f32; 2];
    pub fn mouse_delta(&self) -> [f32; 2];
}
```

- `key_pressed` / `mouse_pressed` — edge-triggered, true only on the transition frame
- `key_held` / `mouse_held` — level-triggered, true while the key/button is down
- `key_released` / `mouse_released` — edge-triggered, true only on the release frame
- `mouse_delta` — accumulated pixel motion since last frame, reset each frame

### Frame lifecycle

1. **Between frames** — Engine accumulates `KeyboardInput`, `MouseInput`, `CursorMoved`, `DeviceEvent::MouseMotion` events into a mutable accumulator
2. **Start of frame** — Engine snapshots: clears `pressed`/`released` sets, resets `mouse_delta`, applies accumulated events
3. **Update** — game reads `engine.input()`, stable for entire frame
4. **After present** — cycle repeats

### Re-export winit key types

Engine re-exports `winit::keyboard::KeyCode` and `winit::event::MouseButton` so games don't need a direct winit dependency for input.

## Flow

1. **Create `input.rs`** in engine — `Input` struct, query methods, internal `begin_frame` / `accumulate` methods.
2. **Add `Input` to `Engine`** — field on Engine, `engine.input()` accessor.
3. **Wire winit events** — `AppRunner::window_event` and `device_event` accumulate into Input.
4. **Snapshot before update** — `Input::begin_frame()` called before the update closure.
5. **Re-export key types** — `pub use winit::keyboard::KeyCode` and `pub use winit::event::MouseButton` from engine.
6. **Add camera to sandbox** — simple FreeCamera using input for WASD + right-click mouse look. The engine already has `FreeCamera` — sandbox uses it.
7. **Test + lint**.

## Decisions

- **HashSet for key tracking** — at most ~10 keys pressed simultaneously. HashSet is simple and fast enough. No bitfield optimization needed.

- **Mouse delta from `DeviceEvent::MouseMotion`** — `CursorMoved` gives absolute position (clipped to window), `MouseMotion` gives raw delta (works even when cursor is at window edge). Use `MouseMotion` for camera look, `CursorMoved` for mouse position.

- **Engine handles escape-to-quit** — this stays in the engine's event handler, not in the input snapshot. Escape is infrastructure, not game logic.

- **No input mapping / action system** — games query raw keys directly. An action mapping layer (bind "jump" to Space) is future work. Raw keys are the foundation it would sit on.

- **Re-export winit types** — games should `use smallworld_engine::input::{KeyCode, MouseButton}`, never `use winit::*` directly. If we switch windowing libraries, the game code doesn't change.
