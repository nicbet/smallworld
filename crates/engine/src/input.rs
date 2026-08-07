//! Frame-captured input snapshot.
//!
//! The [`Engine`](crate::engine::Engine) accumulates winit events between
//! frames and snapshots them into an [`Input`] struct before calling the
//! game's update function. The game polls the snapshot — it's stable for
//! the entire frame.
//!
//! # Key states
//!
//! - **held** — true while the key/button is physically down
//! - **pressed** — true only on the frame the key went down (edge-triggered)
//! - **released** — true only on the frame the key went up (edge-triggered)

use std::collections::HashSet;

pub use winit::event::MouseButton;
pub use winit::keyboard::KeyCode;

/// Per-controller state — numbered axes and buttons.
///
/// Axes and buttons are hardware-indexed, not named. A gamepad maps to
/// axes 0–5 (two sticks + two triggers) and buttons 0–15. A flight stick
/// maps differently. The game or an action mapping layer decides what
/// each index means.
#[derive(Clone, Debug, Default)]
pub struct ControllerState {
    axes: Vec<f32>,
    buttons_held: HashSet<u32>,
    buttons_pressed: HashSet<u32>,
    buttons_released: HashSet<u32>,
}

impl ControllerState {
    /// Reads a numbered axis value (typically -1.0 to 1.0 for sticks,
    /// 0.0 to 1.0 for triggers). Returns 0.0 for nonexistent axes.
    #[must_use]
    pub fn axis(&self, index: usize) -> f32 {
        self.axes.get(index).copied().unwrap_or(0.0)
    }

    /// True while the button is physically down.
    #[must_use]
    pub fn button_held(&self, button: u32) -> bool {
        self.buttons_held.contains(&button)
    }

    /// True only on the frame the button went down.
    #[must_use]
    pub fn button_pressed(&self, button: u32) -> bool {
        self.buttons_pressed.contains(&button)
    }

    /// True only on the frame the button went up.
    #[must_use]
    pub fn button_released(&self, button: u32) -> bool {
        self.buttons_released.contains(&button)
    }
}

/// Frame-captured input snapshot. Stable for the entire update call.
#[derive(Clone, Debug, Default)]
pub struct Input {
    keys_held: HashSet<KeyCode>,
    keys_pressed: HashSet<KeyCode>,
    keys_released: HashSet<KeyCode>,

    mouse_buttons_held: HashSet<MouseButton>,
    mouse_buttons_pressed: HashSet<MouseButton>,
    mouse_buttons_released: HashSet<MouseButton>,
    mouse_position: [f32; 2],
    mouse_delta: [f32; 2],

    controllers: Vec<ControllerState>,
}

impl Input {
    /// True while the key is physically down.
    #[must_use]
    pub fn key_held(&self, key: KeyCode) -> bool {
        self.keys_held.contains(&key)
    }

    /// True only on the frame the key went down.
    #[must_use]
    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.keys_pressed.contains(&key)
    }

    /// True only on the frame the key went up.
    #[must_use]
    pub fn key_released(&self, key: KeyCode) -> bool {
        self.keys_released.contains(&key)
    }

    /// True while the mouse button is physically down.
    #[must_use]
    pub fn mouse_held(&self, button: MouseButton) -> bool {
        self.mouse_buttons_held.contains(&button)
    }

    /// True only on the frame the mouse button went down.
    #[must_use]
    pub fn mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse_buttons_pressed.contains(&button)
    }

    /// True only on the frame the mouse button went up.
    #[must_use]
    pub fn mouse_released(&self, button: MouseButton) -> bool {
        self.mouse_buttons_released.contains(&button)
    }

    /// Cursor position in window-logical pixels.
    #[must_use]
    pub fn mouse_position(&self) -> [f32; 2] {
        self.mouse_position
    }

    /// Raw mouse motion delta since last frame (pixels, unclipped).
    #[must_use]
    pub fn mouse_delta(&self) -> [f32; 2] {
        self.mouse_delta
    }

    /// Access a controller by index. Returns `None` for nonexistent
    /// controllers.
    #[must_use]
    pub fn controller(&self, index: usize) -> Option<&ControllerState> {
        self.controllers.get(index)
    }

    /// Number of connected controllers.
    #[must_use]
    pub fn controller_count(&self) -> usize {
        self.controllers.len()
    }

    /// Prepares the snapshot for a new frame: clears edge-triggered sets
    /// and resets mouse delta. Called by Engine before accumulating events.
    pub(crate) fn begin_frame(&mut self) {
        self.keys_pressed.clear();
        self.keys_released.clear();
        self.mouse_buttons_pressed.clear();
        self.mouse_buttons_released.clear();
        self.mouse_delta = [0.0; 2];
        for c in &mut self.controllers {
            c.buttons_pressed.clear();
            c.buttons_released.clear();
        }
    }

    /// Records a key press/release from a winit keyboard event.
    pub(crate) fn on_keyboard(&mut self, key: KeyCode, pressed: bool) {
        if pressed {
            if self.keys_held.insert(key) {
                self.keys_pressed.insert(key);
            }
        } else {
            self.keys_held.remove(&key);
            self.keys_released.insert(key);
        }
    }

    /// Records a mouse button press/release.
    pub(crate) fn on_mouse_button(&mut self, button: MouseButton, pressed: bool) {
        if pressed {
            if self.mouse_buttons_held.insert(button) {
                self.mouse_buttons_pressed.insert(button);
            }
        } else {
            self.mouse_buttons_held.remove(&button);
            self.mouse_buttons_released.insert(button);
        }
    }

    /// Records cursor position from a winit CursorMoved event.
    pub(crate) fn on_cursor_moved(&mut self, x: f32, y: f32) {
        self.mouse_position = [x, y];
    }

    /// Accumulates raw mouse motion delta.
    pub(crate) fn on_mouse_motion(&mut self, dx: f32, dy: f32) {
        self.mouse_delta[0] += dx;
        self.mouse_delta[1] += dy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_pressed_only_one_frame() {
        let mut input = Input::default();
        input.on_keyboard(KeyCode::KeyW, true);
        assert!(input.key_pressed(KeyCode::KeyW));
        assert!(input.key_held(KeyCode::KeyW));

        input.begin_frame();
        assert!(!input.key_pressed(KeyCode::KeyW));
        assert!(input.key_held(KeyCode::KeyW));
    }

    #[test]
    fn key_released_only_one_frame() {
        let mut input = Input::default();
        input.on_keyboard(KeyCode::KeyW, true);
        input.begin_frame();
        input.on_keyboard(KeyCode::KeyW, false);
        assert!(input.key_released(KeyCode::KeyW));
        assert!(!input.key_held(KeyCode::KeyW));

        input.begin_frame();
        assert!(!input.key_released(KeyCode::KeyW));
    }

    #[test]
    fn mouse_delta_accumulates_and_resets() {
        let mut input = Input::default();
        input.on_mouse_motion(3.0, -2.0);
        input.on_mouse_motion(1.0, 4.0);
        assert_eq!(input.mouse_delta(), [4.0, 2.0]);

        input.begin_frame();
        assert_eq!(input.mouse_delta(), [0.0, 0.0]);
    }

    #[test]
    fn controller_none_for_missing() {
        let input = Input::default();
        assert!(input.controller(0).is_none());
    }
}
