//! Free-fly camera driven by engine input.

use smallworld_engine::camera::FreeCamera;
use smallworld_engine::engine::Engine;
use smallworld_engine::input::{KeyCode, MouseButton};

pub struct CameraRig {
    camera: FreeCamera,
}

impl CameraRig {
    pub fn new() -> Self {
        let mut camera = FreeCamera::new(16.0 / 9.0);
        camera.position = glam::Vec3::new(0.0, 2.0, 5.0);
        Self { camera }
    }

    pub fn update(&mut self, engine: &mut Engine, dt: f32) {
        let input = engine.input();

        let speed = FreeCamera::BASE_SPEED
            * if input.key_held(KeyCode::ShiftLeft) || input.key_held(KeyCode::ShiftRight) {
                FreeCamera::SPRINT_MULTIPLIER
            } else {
                1.0
            }
            * dt;

        let mut delta = glam::Vec3::ZERO;
        if input.key_held(KeyCode::KeyW) {
            delta.z += speed;
        }
        if input.key_held(KeyCode::KeyS) {
            delta.z -= speed;
        }
        if input.key_held(KeyCode::KeyD) {
            delta.x += speed;
        }
        if input.key_held(KeyCode::KeyA) {
            delta.x -= speed;
        }
        if input.key_held(KeyCode::Space) || input.key_held(KeyCode::KeyQ) {
            delta.y += speed;
        }
        if input.key_held(KeyCode::ControlLeft) || input.key_held(KeyCode::KeyE) {
            delta.y -= speed;
        }
        self.camera.translate(delta);

        if input.mouse_held(MouseButton::Right) {
            let [dx, dy] = input.mouse_delta();
            self.camera
                .rotate(dx * FreeCamera::SENSITIVITY, -dy * FreeCamera::SENSITIVITY);
        }

        engine.set_camera(self.camera.position, self.camera.yaw, self.camera.pitch);
    }
}
