//! Volume abstraction for the OOC pipeline.
//!
//! Every voxel data structure in the engine implements [`VoxelVolume`]. The
//! trait exposes enough structure for shader-level traversal optimization —
//! renderers dispatch per [`VolumeKind`] without knowing volume internals,
//! and new volume types plug in without changing the renderer.
//!
//! # Implementor contract
//!
//! - [`volume_kind`](VoxelVolume::volume_kind) must be constant for the
//!   volume's lifetime — the renderer selects a compute kernel based on it.
//! - [`bounds`](VoxelVolume::bounds) returns the world-space AABB enclosing
//!   all solid voxels. Update it when the volume's spatial extent changes.
//! - [`lod_hint`](VoxelVolume::lod_hint) provides metadata for SSE-based LOD
//!   evaluation in the culling stage.
//! - [`traversal_bindings`](VoxelVolume::traversal_bindings) returns GPU
//!   buffers in the order the shader expects for this volume kind. Buffers
//!   must be valid for the current frame.

#![allow(clippy::upper_case_acronyms)]

use glam::Vec3;

/// GPU traversal algorithm discriminant.
///
/// The renderer `match`es on this to select the compute kernel — no vtable
/// dispatch on the hot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeKind {
    /// Sparse Voxel Octree with BrickPool-backed leaves. Cursor-based
    /// front-to-back traversal with occupancy masks.
    SVO,
    /// Uniform 3D grid with simple DDA traversal. For small instanced
    /// props (trees, rocks, pebbles).
    FlatGrid,
    /// 32³ brick chunks in a spatial grid with BVH macro-skip and DDA
    /// micro-traversal. Workhorse for near-field destructible terrain.
    ChunkedDDA,
}

/// GPU-native axis-aligned bounding box, 32 bytes.
///
/// Padded to vec4 boundaries for direct GPU upload. Use this as the
/// canonical AABB type throughout the engine — the AABB buffer
/// expects this layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AABB {
    /// Minimum corner (x, y, z).
    pub min: [f32; 3],
    /// Padding for GPU vec4 alignment.
    pub _pad0: f32,
    /// Maximum corner (x, y, z).
    pub max: [f32; 3],
    /// Padding for GPU vec4 alignment.
    pub _pad1: f32,
}

impl AABB {
    /// Creates an AABB from glam vectors.
    #[must_use]
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self {
            min: min.into(),
            _pad0: 0.0,
            max: max.into(),
            _pad1: 0.0,
        }
    }

    /// Degenerate AABB representing an empty volume.
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY, f32::INFINITY, f32::INFINITY],
        _pad0: 0.0,
        max: [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY],
        _pad1: 0.0,
    };

    /// Returns the minimum corner as a `Vec3`.
    #[must_use]
    pub fn min_vec3(&self) -> Vec3 {
        Vec3::from(self.min)
    }

    /// Returns the maximum corner as a `Vec3`.
    #[must_use]
    pub fn max_vec3(&self) -> Vec3 {
        Vec3::from(self.max)
    }

    /// True if min > max on any axis (degenerate / empty).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1] || self.min[2] > self.max[2]
    }
}

impl From<(Vec3, Vec3)> for AABB {
    fn from((min, max): (Vec3, Vec3)) -> Self {
        Self::new(min, max)
    }
}

/// LOD metadata for SSE-based culling decisions.
///
/// The culling stage computes screen-space error as
/// `voxel_scale × focal_length / distance` and compares against a threshold
/// to decide whether a volume needs full-resolution data or can render at
/// a coarser LOD.
#[derive(Clone, Copy, Debug)]
pub struct LODMeta {
    /// Finest voxel edge length in metres (e.g. 0.1 for terrain, 0.025 for props).
    pub voxel_scale: f32,
    /// Number of LOD levels available (SVO tree depth, or 1 for flat grids).
    pub max_depth: u8,
}

/// GPU buffers required by a renderer to traverse this volume.
///
/// The buffer order is kind-specific and matches the shader's bind group
/// layout for that [`VolumeKind`]:
///
/// - [`SVO`](VolumeKind::SVO) → `[svo_nodes, voxels, palettes, masks]`
/// - [`FlatGrid`](VolumeKind::FlatGrid) → `[grid_data, instances, bvh_nodes]`
/// - [`ChunkedDDA`](VolumeKind::ChunkedDDA) → `[chunk_headers, voxels, palettes, masks, bvh_nodes]`
pub struct TraversalBindings<'a> {
    /// Which traversal kernel these buffers are for.
    pub kind: VolumeKind,
    /// GPU storage buffers in shader bind order.
    pub buffers: &'a [&'a wgpu::Buffer],
}

/// Any spatial voxel data structure the OOC pipeline can resolve, cull,
/// stream, and render.
///
/// Object-safe — can be used as `dyn VoxelVolume` for heterogeneous
/// volume collections.
pub trait VoxelVolume {
    /// The GPU traversal algorithm for this volume. Must be constant for
    /// the volume's lifetime.
    fn volume_kind(&self) -> VolumeKind;

    /// World-space AABB enclosing all solid voxels. Returns [`AABB::EMPTY`]
    /// for empty volumes.
    fn bounds(&self) -> AABB;

    /// LOD metadata for SSE evaluation in the culling stage.
    fn lod_hint(&self) -> LODMeta;

    /// GPU buffers needed for traversal, in the order the shader expects.
    fn traversal_bindings(&self) -> TraversalBindings<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_from_vec3_roundtrip() {
        let min = Vec3::new(1.0, 2.0, 3.0);
        let max = Vec3::new(4.0, 5.0, 6.0);
        let aabb = AABB::new(min, max);
        assert_eq!(aabb.min_vec3(), min);
        assert_eq!(aabb.max_vec3(), max);
    }

    #[test]
    fn aabb_from_tuple() {
        let min = Vec3::ZERO;
        let max = Vec3::ONE;
        let aabb: AABB = (min, max).into();
        assert_eq!(aabb.min_vec3(), min);
        assert_eq!(aabb.max_vec3(), max);
    }

    #[test]
    fn aabb_empty_is_degenerate() {
        assert!(AABB::EMPTY.is_empty());
    }

    #[test]
    fn aabb_valid_is_not_empty() {
        let aabb = AABB::new(Vec3::ZERO, Vec3::ONE);
        assert!(!aabb.is_empty());
    }

    #[test]
    fn aabb_gpu_size() {
        assert_eq!(size_of::<AABB>(), 32);
    }

    #[test]
    fn trait_object_coercion() {
        struct Dummy;
        impl VoxelVolume for Dummy {
            fn volume_kind(&self) -> VolumeKind {
                VolumeKind::FlatGrid
            }
            fn bounds(&self) -> AABB {
                AABB::EMPTY
            }
            fn lod_hint(&self) -> LODMeta {
                LODMeta {
                    voxel_scale: 0.1,
                    max_depth: 1,
                }
            }
            fn traversal_bindings(&self) -> TraversalBindings<'_> {
                TraversalBindings {
                    kind: VolumeKind::FlatGrid,
                    buffers: &[],
                }
            }
        }

        let vol: &dyn VoxelVolume = &Dummy;
        assert_eq!(vol.volume_kind(), VolumeKind::FlatGrid);
        assert!(vol.bounds().is_empty());
    }
}
