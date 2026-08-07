//! Light types for the engine's deferred shading pipeline.
//!
//! Games add lights to the [`World`](crate::world::World) via
//! [`add_light`](crate::world::World::add_light). The pipeline reads
//! them every frame and packs a GPU light buffer — no change tracking
//! needed since the data is small.

use glam::Vec3;

/// Discriminant for light geometry. Each variant carries the spatial
/// parameters specific to that light type.
#[derive(Clone, Debug)]
pub enum LightKind {
    /// Infinitely distant light with uniform direction (sun, moon).
    Directional {
        /// Direction the light travels (normalized, points toward lit surfaces).
        direction: Vec3,
    },
    /// Omnidirectional light with distance falloff.
    Point {
        /// World-space position.
        position: Vec3,
        /// Maximum influence radius in metres.
        range: f32,
    },
    /// Cone-shaped light with angular and distance falloff.
    Spot {
        /// World-space position.
        position: Vec3,
        /// Direction the cone points (normalized).
        direction: Vec3,
        /// Maximum influence radius in metres.
        range: f32,
        /// Half-angle of the full-brightness inner cone (radians).
        inner_angle: f32,
        /// Half-angle of the falloff-to-zero outer cone (radians).
        outer_angle: f32,
    },
}

/// A light source in the scene.
#[derive(Clone, Debug)]
pub struct Light {
    /// Spatial type and geometry.
    pub kind: LightKind,
    /// Linear-space RGB color.
    pub color: Vec3,
    /// Luminous intensity (lux for directional, candela for point/spot).
    pub intensity: f32,
    /// Whether this light writes to the shadow atlas.
    pub casts_shadows: bool,
}

impl Light {
    /// Creates a directional light (sun). Shadows enabled by default.
    #[must_use]
    pub fn directional(direction: Vec3, color: Vec3, intensity: f32) -> Self {
        Self {
            kind: LightKind::Directional {
                direction: direction.normalize(),
            },
            color,
            intensity,
            casts_shadows: true,
        }
    }

    /// Creates a point light. Shadows disabled by default.
    #[must_use]
    pub fn point(position: Vec3, range: f32, color: Vec3, intensity: f32) -> Self {
        Self {
            kind: LightKind::Point { position, range },
            color,
            intensity,
            casts_shadows: false,
        }
    }

    /// Creates a spot light. Shadows disabled by default.
    #[must_use]
    pub fn spot(
        position: Vec3,
        direction: Vec3,
        range: f32,
        inner_angle: f32,
        outer_angle: f32,
        color: Vec3,
        intensity: f32,
    ) -> Self {
        Self {
            kind: LightKind::Spot {
                position,
                direction: direction.normalize(),
                range,
                inner_angle,
                outer_angle,
            },
            color,
            intensity,
            casts_shadows: false,
        }
    }
}
