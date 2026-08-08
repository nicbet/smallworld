//! Mesh geometry and placement types for the rasterization path.
//!
//! [`Mesh`] holds shared vertex/index data (the asset).
//! [`MeshInstance`] places a mesh in the world with a transform and
//! material reference. Both are stored in the
//! [`World`](crate::world::World) via their respective `add_*` methods.

use glam::{Quat, Vec3};

use crate::volume::AABB;
use crate::world::{MaterialKey, MeshKey};

/// PBR-ready vertex. Matches the GBuffer input layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Object-space normal (unit length).
    pub normal: [f32; 3],
    /// Texture coordinates.
    pub uv: [f32; 2],
    /// Tangent vector (xyz) + bitangent handedness (w = +1 or -1).
    pub tangent: [f32; 4],
}

/// Shared mesh geometry. Multiple [`MeshInstance`]s can reference the
/// same `Mesh`.
pub struct Mesh {
    /// Vertex data.
    pub vertices: Vec<Vertex>,
    /// Triangle index data (three indices per triangle).
    pub indices: Vec<u32>,
    /// Object-space bounding box computed from vertices.
    pub bounds: AABB,
}

impl Mesh {
    /// Creates a mesh and computes its AABB from vertex positions.
    #[must_use]
    pub fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        let bounds = compute_bounds(&vertices);
        Self {
            vertices,
            indices,
            bounds,
        }
    }
}

/// A placed mesh in the world: references shared geometry and material,
/// positioned by transform.
pub struct MeshInstance {
    /// Handle to the shared [`Mesh`] in the World.
    pub mesh: MeshKey,
    /// Handle to the shared [`Material`](crate::material::Material).
    pub material: MaterialKey,
    /// World-space position.
    pub position: Vec3,
    /// Orientation.
    pub rotation: Quat,
    /// Non-uniform scale.
    pub scale: Vec3,
    /// Whether this instance writes to the shadow atlas.
    pub casts_shadows: bool,
    /// Whether both sides of triangles are rendered (disables backface culling).
    pub double_sided: bool,
}

fn compute_bounds(vertices: &[Vertex]) -> AABB {
    if vertices.is_empty() {
        return AABB::EMPTY;
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in vertices {
        let p = Vec3::from(v.position);
        min = min.min(p);
        max = max.max(p);
    }
    AABB::new(min, max)
}
