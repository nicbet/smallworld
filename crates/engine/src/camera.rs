//! Free-fly camera — pure math, no input handling.

use glam::Vec3;

type Mat4 = glam::Mat4;

/// A free-fly camera with position, yaw, and pitch.
pub struct FreeCamera {
    /// World-space position.
    pub position: Vec3,
    /// Horizontal rotation in radians (0 = -Z).
    pub yaw: f32,
    /// Vertical rotation in radians, clamped to ±89°.
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    /// Viewport aspect ratio (width / height).
    pub aspect: f32,
    /// Near clip plane.
    pub near: f32,
    /// Far clip plane.
    pub far: f32,
}

impl FreeCamera {
    /// Base movement speed in metres per second.
    pub const BASE_SPEED: f32 = 5.0;
    /// Sprint multiplier.
    pub const SPRINT_MULTIPLIER: f32 = 4.0;
    /// Mouse sensitivity in radians per pixel.
    pub const SENSITIVITY: f32 = 0.003;

    const PITCH_LIMIT: f32 = 89.0_f32 * (std::f32::consts::PI / 180.0);

    /// Creates a camera at the origin looking along -Z.
    #[must_use]
    pub fn new(aspect: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 2.0, 5.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60.0_f32.to_radians(),
            aspect,
            near: 0.1,
            far: 1000.0,
        }
    }

    /// Unit vector pointing where the camera faces.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -(self.yaw.cos() * self.pitch.cos()),
        )
        .normalize()
    }

    /// Unit vector pointing right relative to the camera.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize()
    }

    /// View matrix (world → camera).
    #[must_use]
    pub fn view_matrix(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.position, self.position + self.forward(), Vec3::Y)
    }

    /// Projection matrix (camera → clip).
    #[must_use]
    pub fn projection_matrix(&self) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(self.fov_y, self.aspect, self.near, self.far)
    }

    /// Translates the camera in its local coordinate frame.
    pub fn translate(&mut self, local_delta: Vec3) {
        let fwd = self.forward();
        let right = self.right();
        self.position += right * local_delta.x + Vec3::Y * local_delta.y + fwd * local_delta.z;
    }

    /// Rotates the camera by the given yaw/pitch deltas (radians).
    pub fn rotate(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw += yaw_delta;
        self.pitch = (self.pitch + pitch_delta).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_forward_is_negative_z() {
        let cam = FreeCamera::new(16.0 / 9.0);
        let fwd = cam.forward();
        assert!(fwd.x.abs() < 1e-5);
        assert!(fwd.y.abs() < 1e-5);
        assert!((fwd.z + 1.0).abs() < 1e-5, "expected -Z, got {fwd:?}");
    }

    #[test]
    fn pitch_is_clamped() {
        let mut cam = FreeCamera::new(1.0);
        cam.rotate(0.0, 100.0);
        assert!(cam.pitch <= FreeCamera::PITCH_LIMIT + 1e-5);
        cam.rotate(0.0, -200.0);
        assert!(cam.pitch >= -FreeCamera::PITCH_LIMIT - 1e-5);
    }

    #[test]
    fn view_matrix_is_invertible() {
        let cam = FreeCamera::new(16.0 / 9.0);
        let det = cam.view_matrix().determinant();
        assert!(det.abs() > 1e-6, "degenerate view matrix");
    }
}
