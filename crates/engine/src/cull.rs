//! Cull stage: first stage of the OOC pipeline.
//!
//! Takes the [`World`] and view parameters, produces a [`VisibilitySet`] —
//! the subset of volumes, mesh instances, and lights that need rendering
//! this frame. Downstream stages (Stream, Execute) operate on the visible
//! set, never the full world.
//!
//! Current implementation is a no-op passthrough (everything visible).
//! The interface accepts an optional HZB texture for future GPU occlusion
//! culling.

use crate::engine::ViewState;
use crate::world::{LightKey, MeshInstanceKey, VolumeKey, World};

/// The output of the cull stage: keys for everything visible this frame.
pub struct VisibilitySet {
    /// Visible voxel volumes.
    pub volumes: Vec<VolumeKey>,
    /// Visible mesh instances.
    pub mesh_instances: Vec<MeshInstanceKey>,
    /// Visible lights (directional lights are always included).
    pub lights: Vec<LightKey>,
}

impl VisibilitySet {
    /// Total number of visible objects across all categories.
    #[must_use]
    pub fn total(&self) -> usize {
        self.volumes.len() + self.mesh_instances.len() + self.lights.len()
    }

    /// True if nothing is visible.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty() && self.mesh_instances.is_empty() && self.lights.is_empty()
    }
}

/// First pipeline stage. Determines what's visible from the current
/// viewpoint.
#[derive(Default)]
pub struct CullStage;

impl CullStage {
    /// Creates a new cull stage.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Determines visibility for the current frame.
    ///
    /// - `world` — scene data (volumes, mesh instances, lights)
    /// - `_view` — camera position, orientation, FOV (unused in no-op)
    /// - `_hzb` — hierarchical-Z buffer from previous frame for occlusion
    ///   culling (None until the HZB builder lands)
    ///
    /// Current implementation: everything visible (passthrough).
    pub fn cull(
        &self,
        world: &World,
        _view: &ViewState,
        _hzb: Option<&wgpu::TextureView>,
    ) -> VisibilitySet {
        let volumes: Vec<VolumeKey> = world.volumes().map(|(k, _)| k).collect();
        let mesh_instances: Vec<MeshInstanceKey> =
            world.mesh_instances().map(|(k, _)| k).collect();
        let lights: Vec<LightKey> = world.lights().map(|(k, _)| k).collect();

        VisibilitySet {
            volumes,
            mesh_instances,
            lights,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::Light;
    use crate::material::Material;
    use crate::mesh::{Mesh, MeshInstance, Vertex};
    use crate::volume::{AABB, LODMeta, TraversalBindings, VolumeKind, VoxelVolume};
    use glam::{Quat, Vec3};

    struct DummyVolume;
    impl VoxelVolume for DummyVolume {
        fn volume_kind(&self) -> VolumeKind {
            VolumeKind::FlatGrid
        }
        fn bounds(&self) -> AABB {
            AABB::new(Vec3::ZERO, Vec3::ONE)
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

    fn populated_world() -> World {
        let mut world = World::new();

        world.add_volume(Box::new(DummyVolume));
        world.add_volume(Box::new(DummyVolume));

        let mat = world.add_material(Material::default());
        let mesh = world.add_mesh(Mesh::new(
            vec![Vertex {
                position: [0.0, 0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            }],
            vec![0],
        ));
        world.add_mesh_instance(MeshInstance {
            mesh,
            material: mat,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            casts_shadows: true,
        });

        world.add_light(Light::directional(-Vec3::Y, Vec3::ONE, 1.0));
        world.add_light(Light::point(Vec3::ZERO, 10.0, Vec3::ONE, 5.0));

        world
    }

    #[test]
    fn empty_world_produces_empty_visibility() {
        let world = World::new();
        let cull = CullStage::new();
        let vis = cull.cull(&world, &ViewState::default(), None);
        assert!(vis.is_empty());
        assert_eq!(vis.total(), 0);
    }

    #[test]
    fn noop_includes_everything() {
        let world = populated_world();
        let cull = CullStage::new();
        let vis = cull.cull(&world, &ViewState::default(), None);

        assert_eq!(vis.volumes.len(), world.volume_count());
        assert_eq!(vis.mesh_instances.len(), world.mesh_instance_count());
        assert_eq!(vis.lights.len(), world.light_count());
        assert_eq!(vis.total(), 5);
    }

    #[test]
    fn visibility_keys_match_world_keys() {
        let world = populated_world();
        let cull = CullStage::new();
        let vis = cull.cull(&world, &ViewState::default(), None);

        for key in &vis.volumes {
            assert!(world.volume(*key).is_some());
        }
        for key in &vis.mesh_instances {
            assert!(world.mesh_instance(*key).is_some());
        }
        for key in &vis.lights {
            assert!(world.light(*key).is_some());
        }
    }
}
