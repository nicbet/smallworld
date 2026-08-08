//! World: the engine's central scene container.
//!
//! Holds everything the pipeline renders: voxel volumes (raymarched),
//! mesh instances (rasterized), shared materials, and lights. Games
//! populate the World; pipeline stages consume it.
//!
//! Per-object change tracking via [`ChangeSet`] lets the pipeline
//! process only what changed each frame. Lights are excluded — they are
//! re-packed into a small GPU buffer every frame.

use crate::light::Light;
use crate::material::Material;
use crate::mesh::{Mesh, MeshInstance};
use crate::texture::TextureData;
use crate::volume::VoxelVolume;

slotmap::new_key_type! {
    /// Stable handle to a [`VoxelVolume`] in the World.
    pub struct VolumeKey;
    /// Stable handle to a shared [`Mesh`] asset in the World.
    pub struct MeshKey;
    /// Stable handle to a placed [`MeshInstance`] in the World.
    pub struct MeshInstanceKey;
    /// Stable handle to a shared [`Material`] in the World.
    pub struct MaterialKey;
    /// Stable handle to a [`Light`] in the World.
    pub struct LightKey;
    /// Stable handle to a [`TextureData`] in the World.
    pub struct TextureKey;
}

// ---------------------------------------------------------------------------
// Change tracking
// ---------------------------------------------------------------------------

pub(crate) struct ChangeSet<K: slotmap::Key> {
    spawned: Vec<K>,
    despawned: Vec<K>,
    mutated: Vec<K>,
}

impl<K: slotmap::Key> ChangeSet<K> {
    fn new() -> Self {
        Self {
            spawned: Vec::new(),
            despawned: Vec::new(),
            mutated: Vec::new(),
        }
    }

    fn mark_spawned(&mut self, key: K) {
        self.spawned.push(key);
    }

    fn mark_despawned(&mut self, key: K) {
        self.despawned.push(key);
    }

    fn mark_mutated(&mut self, key: K) {
        self.mutated.push(key);
    }

    fn drain(&mut self) -> DrainedChanges<K> {
        DrainedChanges {
            spawned: std::mem::take(&mut self.spawned),
            despawned: std::mem::take(&mut self.despawned),
            mutated: std::mem::take(&mut self.mutated),
        }
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.spawned.is_empty() && self.despawned.is_empty() && self.mutated.is_empty()
    }
}

#[allow(dead_code)]
pub(crate) struct DrainedChanges<K: slotmap::Key> {
    pub spawned: Vec<K>,
    pub despawned: Vec<K>,
    pub mutated: Vec<K>,
}

// ---------------------------------------------------------------------------
// WorldChanges — returned by drain_changes()
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) struct WorldChanges {
    pub volumes: DrainedChanges<VolumeKey>,
    pub meshes: DrainedChanges<MeshKey>,
    pub mesh_instances: DrainedChanges<MeshInstanceKey>,
    pub materials: DrainedChanges<MaterialKey>,
}

// ---------------------------------------------------------------------------
// WorldGpuData — stub until pipeline stages land
// ---------------------------------------------------------------------------

/// GPU-ready snapshot of world data. Stub — will be repopulated by
/// pipeline stages (Cull/Stream/Execute) in later issues.
pub struct WorldGpuData {
    instance_buf: Option<wgpu::Buffer>,
    grid_buf: Option<wgpu::Buffer>,
    bvh_buf: Option<wgpu::Buffer>,
    instance_count: u32,
    generation: u32,
}

impl WorldGpuData {
    /// An empty snapshot — no instances, no buffers.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            instance_buf: None,
            grid_buf: None,
            bvh_buf: None,
            instance_count: 0,
            generation: 0,
        }
    }

    /// The packed instance data buffer.
    #[must_use]
    pub fn instance_buffer(&self) -> Option<&wgpu::Buffer> {
        self.instance_buf.as_ref()
    }

    /// The packed object grid buffer.
    #[must_use]
    pub fn grid_buffer(&self) -> Option<&wgpu::Buffer> {
        self.grid_buf.as_ref()
    }

    /// The BVH node buffer.
    #[must_use]
    pub fn bvh_buffer(&self) -> Option<&wgpu::Buffer> {
        self.bvh_buf.as_ref()
    }

    /// Number of instances in the snapshot.
    #[must_use]
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }

    /// Monotonically increasing counter, bumped on each rebuild.
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    #[allow(dead_code)]
    pub(crate) fn extract(_device: &wgpu::Device, _world: &World) -> Self {
        Self::empty()
    }
}

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

/// The engine's central scene container. Hybrid: voxel volumes
/// (raymarched) + mesh instances (rasterized) + shared materials + lights.
///
/// Games populate a World and hand it to the [`Engine`](crate::engine::Engine).
/// Pipeline stages consume it each frame via per-object change tracking.
pub struct World {
    volumes: slotmap::SlotMap<VolumeKey, Box<dyn VoxelVolume>>,
    volume_changes: ChangeSet<VolumeKey>,

    meshes: slotmap::SlotMap<MeshKey, Mesh>,
    mesh_changes: ChangeSet<MeshKey>,

    mesh_instances: slotmap::SlotMap<MeshInstanceKey, MeshInstance>,
    mesh_instance_changes: ChangeSet<MeshInstanceKey>,

    materials: slotmap::SlotMap<MaterialKey, Material>,
    material_changes: ChangeSet<MaterialKey>,

    textures: slotmap::SlotMap<TextureKey, TextureData>,

    lights: slotmap::SlotMap<LightKey, Light>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// Creates an empty world.
    #[must_use]
    pub fn new() -> Self {
        Self {
            volumes: slotmap::SlotMap::with_key(),
            volume_changes: ChangeSet::new(),
            meshes: slotmap::SlotMap::with_key(),
            mesh_changes: ChangeSet::new(),
            mesh_instances: slotmap::SlotMap::with_key(),
            mesh_instance_changes: ChangeSet::new(),
            materials: slotmap::SlotMap::with_key(),
            material_changes: ChangeSet::new(),
            textures: slotmap::SlotMap::with_key(),
            lights: slotmap::SlotMap::with_key(),
        }
    }

    // -- Volumes ----------------------------------------------------------

    /// Adds a voxel volume and returns its stable handle.
    pub fn add_volume(&mut self, volume: Box<dyn VoxelVolume>) -> VolumeKey {
        let key = self.volumes.insert(volume);
        self.volume_changes.mark_spawned(key);
        key
    }

    /// Removes a volume by key. Returns it if it existed.
    pub fn remove_volume(&mut self, key: VolumeKey) -> Option<Box<dyn VoxelVolume>> {
        let removed = self.volumes.remove(key);
        if removed.is_some() {
            self.volume_changes.mark_despawned(key);
        }
        removed
    }

    /// Read access to a volume by key.
    #[must_use]
    pub fn volume(&self, key: VolumeKey) -> Option<&dyn VoxelVolume> {
        self.volumes.get(key).map(|v| v.as_ref())
    }

    /// Iterates all volumes.
    pub fn volumes(&self) -> impl Iterator<Item = (VolumeKey, &dyn VoxelVolume)> {
        self.volumes.iter().map(|(k, v)| (k, v.as_ref()))
    }

    /// Number of volumes in the world.
    #[must_use]
    pub fn volume_count(&self) -> usize {
        self.volumes.len()
    }

    // -- Meshes (shared geometry assets) ----------------------------------

    /// Registers a shared mesh asset and returns its handle.
    pub fn add_mesh(&mut self, mesh: Mesh) -> MeshKey {
        let key = self.meshes.insert(mesh);
        self.mesh_changes.mark_spawned(key);
        key
    }

    /// Removes a mesh asset by key.
    pub fn remove_mesh(&mut self, key: MeshKey) -> Option<Mesh> {
        let removed = self.meshes.remove(key);
        if removed.is_some() {
            self.mesh_changes.mark_despawned(key);
        }
        removed
    }

    /// Read access to a mesh asset by key.
    #[must_use]
    pub fn mesh(&self, key: MeshKey) -> Option<&Mesh> {
        self.meshes.get(key)
    }

    /// Iterates all mesh assets.
    pub fn meshes(&self) -> impl Iterator<Item = (MeshKey, &Mesh)> {
        self.meshes.iter()
    }

    /// Number of mesh assets in the world.
    #[must_use]
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    // -- Mesh instances (placed objects) ----------------------------------

    /// Places a mesh instance in the world and returns its handle.
    pub fn add_mesh_instance(&mut self, instance: MeshInstance) -> MeshInstanceKey {
        let key = self.mesh_instances.insert(instance);
        self.mesh_instance_changes.mark_spawned(key);
        key
    }

    /// Removes a mesh instance by key.
    pub fn remove_mesh_instance(&mut self, key: MeshInstanceKey) -> Option<MeshInstance> {
        let removed = self.mesh_instances.remove(key);
        if removed.is_some() {
            self.mesh_instance_changes.mark_despawned(key);
        }
        removed
    }

    /// Read access to a mesh instance by key.
    #[must_use]
    pub fn mesh_instance(&self, key: MeshInstanceKey) -> Option<&MeshInstance> {
        self.mesh_instances.get(key)
    }

    /// Mutable access to a mesh instance. Records the key as mutated.
    pub fn mesh_instance_mut(&mut self, key: MeshInstanceKey) -> Option<&mut MeshInstance> {
        if self.mesh_instances.contains_key(key) {
            self.mesh_instance_changes.mark_mutated(key);
        }
        self.mesh_instances.get_mut(key)
    }

    /// Iterates all mesh instances.
    pub fn mesh_instances(&self) -> impl Iterator<Item = (MeshInstanceKey, &MeshInstance)> {
        self.mesh_instances.iter()
    }

    /// Number of mesh instances in the world.
    #[must_use]
    pub fn mesh_instance_count(&self) -> usize {
        self.mesh_instances.len()
    }

    // -- Materials (shared) -----------------------------------------------

    /// Registers a shared material and returns its handle.
    pub fn add_material(&mut self, material: Material) -> MaterialKey {
        let key = self.materials.insert(material);
        self.material_changes.mark_spawned(key);
        key
    }

    /// Removes a material by key.
    pub fn remove_material(&mut self, key: MaterialKey) -> Option<Material> {
        let removed = self.materials.remove(key);
        if removed.is_some() {
            self.material_changes.mark_despawned(key);
        }
        removed
    }

    /// Read access to a material by key.
    #[must_use]
    pub fn material(&self, key: MaterialKey) -> Option<&Material> {
        self.materials.get(key)
    }

    /// Mutable access to a material. Records the key as mutated.
    pub fn material_mut(&mut self, key: MaterialKey) -> Option<&mut Material> {
        if self.materials.contains_key(key) {
            self.material_changes.mark_mutated(key);
        }
        self.materials.get_mut(key)
    }

    /// Iterates all materials.
    pub fn materials(&self) -> impl Iterator<Item = (MaterialKey, &Material)> {
        self.materials.iter()
    }

    /// Number of materials in the world.
    #[must_use]
    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    // -- Textures (immutable after creation, no change tracking) ----------

    /// Registers a texture and returns its handle.
    pub fn add_texture(&mut self, texture: TextureData) -> TextureKey {
        self.textures.insert(texture)
    }

    /// Read access to a texture by key.
    #[must_use]
    pub fn texture(&self, key: TextureKey) -> Option<&TextureData> {
        self.textures.get(key)
    }

    /// Number of textures in the world.
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    // -- Lights (no change tracking — re-packed per frame) ----------------

    /// Adds a light and returns its handle.
    pub fn add_light(&mut self, light: Light) -> LightKey {
        self.lights.insert(light)
    }

    /// Removes a light by key.
    pub fn remove_light(&mut self, key: LightKey) -> Option<Light> {
        self.lights.remove(key)
    }

    /// Read access to a light by key.
    #[must_use]
    pub fn light(&self, key: LightKey) -> Option<&Light> {
        self.lights.get(key)
    }

    /// Mutable access to a light. No change tracking — lights are
    /// re-packed into the GPU buffer every frame.
    pub fn light_mut(&mut self, key: LightKey) -> Option<&mut Light> {
        self.lights.get_mut(key)
    }

    /// Iterates all lights.
    pub fn lights(&self) -> impl Iterator<Item = (LightKey, &Light)> {
        self.lights.iter()
    }

    /// Number of lights in the world.
    #[must_use]
    pub fn light_count(&self) -> usize {
        self.lights.len()
    }

    // -- Change tracking --------------------------------------------------

    pub(crate) fn drain_changes(&mut self) -> WorldChanges {
        WorldChanges {
            volumes: self.volume_changes.drain(),
            meshes: self.mesh_changes.drain(),
            mesh_instances: self.mesh_instance_changes.drain(),
            materials: self.material_changes.drain(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volume::{AABB, LODMeta, TraversalBindings, VolumeKind};
    use glam::{Quat, Vec3, Vec4};

    struct DummyVolume;
    impl VoxelVolume for DummyVolume {
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

    fn test_material() -> Material {
        Material {
            base_color: Vec4::ONE,
            roughness: 0.5,
            metallic: 0.0,
            emissive: Vec3::ZERO,
            ..Material::default()
        }
    }

    fn test_mesh() -> Mesh {
        Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            bounds: AABB::EMPTY,
        }
    }

    fn test_mesh_instance(mesh: MeshKey, material: MaterialKey) -> MeshInstance {
        MeshInstance {
            mesh,
            material,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            casts_shadows: true,
            double_sided: false,
        }
    }

    // -- Volumes --

    #[test]
    fn volume_add_remove() {
        let mut world = World::new();
        let key = world.add_volume(Box::new(DummyVolume));
        assert_eq!(world.volume_count(), 1);
        assert!(world.volume(key).is_some());

        let removed = world.remove_volume(key);
        assert!(removed.is_some());
        assert_eq!(world.volume_count(), 0);
        assert!(world.volume(key).is_none());
    }

    #[test]
    fn volume_changes_tracked() {
        let mut world = World::new();
        let key = world.add_volume(Box::new(DummyVolume));

        let changes = world.drain_changes();
        assert_eq!(changes.volumes.spawned.len(), 1);
        assert_eq!(changes.volumes.spawned[0], key);

        world.remove_volume(key);
        let changes = world.drain_changes();
        assert_eq!(changes.volumes.despawned.len(), 1);
        assert_eq!(changes.volumes.despawned[0], key);
    }

    #[test]
    fn volume_iterate() {
        let mut world = World::new();
        world.add_volume(Box::new(DummyVolume));
        world.add_volume(Box::new(DummyVolume));
        let vols: Vec<_> = world.volumes().collect();
        assert_eq!(vols.len(), 2);
    }

    #[test]
    fn volume_trait_object_coercion() {
        let mut world = World::new();
        let key = world.add_volume(Box::new(DummyVolume));
        let vol = world.volume(key).unwrap();
        assert_eq!(vol.volume_kind(), VolumeKind::FlatGrid);
        assert!(vol.bounds().is_empty());
    }

    // -- Materials --

    #[test]
    fn material_add_remove() {
        let mut world = World::new();
        let key = world.add_material(test_material());
        assert_eq!(world.material_count(), 1);
        assert!(world.material(key).is_some());

        let removed = world.remove_material(key);
        assert!(removed.is_some());
        assert_eq!(world.material_count(), 0);
    }

    #[test]
    fn material_changes_tracked() {
        let mut world = World::new();
        let key = world.add_material(test_material());

        let changes = world.drain_changes();
        assert_eq!(changes.materials.spawned.len(), 1);

        world.material_mut(key).unwrap().roughness = 1.0;
        let changes = world.drain_changes();
        assert_eq!(changes.materials.mutated.len(), 1);
        assert_eq!(changes.materials.mutated[0], key);

        world.remove_material(key);
        let changes = world.drain_changes();
        assert_eq!(changes.materials.despawned.len(), 1);
    }

    // -- Meshes --

    #[test]
    fn mesh_add_remove() {
        let mut world = World::new();
        let key = world.add_mesh(test_mesh());
        assert_eq!(world.mesh_count(), 1);
        assert!(world.mesh(key).is_some());

        let removed = world.remove_mesh(key);
        assert!(removed.is_some());
        assert_eq!(world.mesh_count(), 0);
    }

    #[test]
    fn mesh_changes_tracked() {
        let mut world = World::new();
        let key = world.add_mesh(test_mesh());

        let changes = world.drain_changes();
        assert_eq!(changes.meshes.spawned.len(), 1);
        assert_eq!(changes.meshes.spawned[0], key);

        world.remove_mesh(key);
        let changes = world.drain_changes();
        assert_eq!(changes.meshes.despawned.len(), 1);
    }

    // -- Mesh instances --

    #[test]
    fn mesh_instance_add_remove() {
        let mut world = World::new();
        let mesh = world.add_mesh(test_mesh());
        let mat = world.add_material(test_material());
        let inst = world.add_mesh_instance(test_mesh_instance(mesh, mat));
        assert_eq!(world.mesh_instance_count(), 1);
        assert!(world.mesh_instance(inst).is_some());

        let removed = world.remove_mesh_instance(inst);
        assert!(removed.is_some());
        assert_eq!(world.mesh_instance_count(), 0);
    }

    #[test]
    fn mesh_instance_changes_tracked() {
        let mut world = World::new();
        let mesh = world.add_mesh(test_mesh());
        let mat = world.add_material(test_material());
        let inst = world.add_mesh_instance(test_mesh_instance(mesh, mat));

        let changes = world.drain_changes();
        assert_eq!(changes.mesh_instances.spawned.len(), 1);

        world.mesh_instance_mut(inst).unwrap().position = Vec3::new(1.0, 2.0, 3.0);
        let changes = world.drain_changes();
        assert_eq!(changes.mesh_instances.mutated.len(), 1);
        assert_eq!(changes.mesh_instances.mutated[0], inst);

        world.remove_mesh_instance(inst);
        let changes = world.drain_changes();
        assert_eq!(changes.mesh_instances.despawned.len(), 1);
    }

    // -- Lights --

    #[test]
    fn light_add_remove() {
        let mut world = World::new();
        let sun = world.add_light(Light::directional(
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::ONE,
            1.0,
        ));
        assert_eq!(world.light_count(), 1);
        assert!(world.light(sun).is_some());

        let removed = world.remove_light(sun);
        assert!(removed.is_some());
        assert_eq!(world.light_count(), 0);
    }

    #[test]
    fn light_mut_no_change_tracking() {
        let mut world = World::new();
        let key = world.add_light(Light::point(Vec3::ZERO, 10.0, Vec3::ONE, 5.0));

        world.drain_changes();

        world.light_mut(key).unwrap().intensity = 100.0;

        let changes = world.drain_changes();
        assert!(changes.volumes.spawned.is_empty());
        assert!(changes.meshes.spawned.is_empty());
        assert!(changes.mesh_instances.spawned.is_empty());
        assert!(changes.materials.spawned.is_empty());
    }

    #[test]
    fn light_iterate() {
        let mut world = World::new();
        world.add_light(Light::directional(-Vec3::Y, Vec3::ONE, 1.0));
        world.add_light(Light::point(Vec3::ZERO, 10.0, Vec3::ONE, 5.0));
        world.add_light(Light::spot(
            Vec3::Y,
            -Vec3::Y,
            20.0,
            0.3,
            0.5,
            Vec3::ONE,
            10.0,
        ));
        let lights: Vec<_> = world.lights().collect();
        assert_eq!(lights.len(), 3);
    }

    // -- Drain clears --

    #[test]
    fn drain_clears_all_changes() {
        let mut world = World::new();
        let v = world.add_volume(Box::new(DummyVolume));
        let m = world.add_mesh(test_mesh());
        let mat = world.add_material(test_material());
        let _inst = world.add_mesh_instance(test_mesh_instance(m, mat));

        let changes = world.drain_changes();
        assert!(!changes.volumes.spawned.is_empty());

        assert!(world.volume_changes.is_empty());
        assert!(world.mesh_changes.is_empty());
        assert!(world.mesh_instance_changes.is_empty());
        assert!(world.material_changes.is_empty());

        let _ = v;
    }

    // -- Empty world --

    #[test]
    fn empty_world_is_valid() {
        let mut world = World::new();
        assert_eq!(world.volume_count(), 0);
        assert_eq!(world.mesh_count(), 0);
        assert_eq!(world.mesh_instance_count(), 0);
        assert_eq!(world.material_count(), 0);
        assert_eq!(world.light_count(), 0);

        let changes = world.drain_changes();
        assert!(changes.volumes.spawned.is_empty());
    }
}
