//! PBR material properties shared across mesh instances.
//!
//! Materials are stored in the [`World`](crate::world::World) and
//! referenced by [`MeshInstance`](crate::mesh::MeshInstance) via
//! [`MaterialKey`](crate::world::MaterialKey). Multiple instances can
//! share one material — changing a material's properties updates all
//! objects that reference it.

use glam::{Vec3, Vec4};

/// Scalar PBR material. Texture maps are future scope.
#[derive(Clone, Debug)]
pub struct Material {
    /// Base color (RGB) and opacity (A). Linear space.
    pub base_color: Vec4,
    /// Surface roughness `[0.0, 1.0]`. 0 = mirror, 1 = diffuse.
    pub roughness: f32,
    /// Metallic factor `[0.0, 1.0]`. 0 = dielectric, 1 = conductor.
    pub metallic: f32,
    /// Emissive radiance (linear RGB). Adds light independent of illumination.
    pub emissive: Vec3,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: Vec4::new(1.0, 1.0, 1.0, 1.0),
            roughness: 0.5,
            metallic: 0.0,
            emissive: Vec3::ZERO,
        }
    }
}
