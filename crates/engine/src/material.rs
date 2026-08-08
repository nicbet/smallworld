//! PBR material properties shared across mesh instances.
//!
//! Materials are stored in the [`World`](crate::world::World) and
//! referenced by [`MeshInstance`](crate::mesh::MeshInstance) via
//! [`MaterialKey`](crate::world::MaterialKey). Multiple instances can
//! share one material — changing a material's properties updates all
//! objects that reference it.

use glam::{Vec3, Vec4};

use crate::world::TextureKey;

/// PBR material with optional texture maps.
#[derive(Clone, Debug)]
pub struct Material {
    /// Base color (RGB) and opacity (A). Linear space.
    /// Multiplied with `albedo_map` when present.
    pub base_color: Vec4,
    /// Surface roughness `[0.0, 1.0]`. 0 = mirror, 1 = diffuse.
    /// Multiplied with `roughness_metallic_map` green channel when present.
    pub roughness: f32,
    /// Metallic factor `[0.0, 1.0]`. 0 = dielectric, 1 = conductor.
    /// Multiplied with `roughness_metallic_map` blue channel when present.
    pub metallic: f32,
    /// Emissive radiance (linear RGB). Adds light independent of illumination.
    pub emissive: Vec3,
    /// Albedo texture. Sampled and multiplied with `base_color`.
    pub albedo_map: Option<TextureKey>,
    /// Tangent-space normal map.
    pub normal_map: Option<TextureKey>,
    /// Roughness (green) + metallic (blue) packed texture (glTF convention).
    pub roughness_metallic_map: Option<TextureKey>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: Vec4::new(1.0, 1.0, 1.0, 1.0),
            roughness: 0.5,
            metallic: 0.0,
            emissive: Vec3::ZERO,
            albedo_map: None,
            normal_map: None,
            roughness_metallic_map: None,
        }
    }
}
