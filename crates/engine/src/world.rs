//! World: the engine's central data structure for instanced voxel objects.
//!
//! Pure CPU data — no GPU dependency. Games construct a [`World`], populate it
//! with models and instances, and hand it to the [`Engine`](crate::engine::Engine)
//! which handles extraction to GPU buffers automatically.

use crate::bvh;
use crate::volume::AABB;
use crate::voxel_object::{VoxelInstance, VoxelInstanceGpu, VoxelModel};

slotmap::new_key_type! {
    /// Stable handle to an instance in the [`World`].
    pub struct InstanceKey;
}

/// GPU-ready snapshot of world data. Produced by the engine's extraction
/// step, consumed by the renderer. The game never constructs this directly.
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

    /// Extracts world data into GPU buffers. Called by Engine, not by games.
    pub(crate) fn extract(device: &wgpu::Device, world: &World) -> Self {
        if world.instances.is_empty() {
            return Self::empty();
        }

        let instances: Vec<&VoxelInstance> = world.instances.values().collect();

        // Pack model grids
        let mut grid_offsets: Vec<u32> = Vec::with_capacity(world.models.len());
        let mut packed_grids: Vec<u32> = Vec::new();
        for model in &world.models {
            grid_offsets.push(packed_grids.len() as u32);
            packed_grids.extend_from_slice(model.grid_data());
        }

        // Build AABBs for BVH
        let aabbs: Vec<AABB> = instances
            .iter()
            .map(|inst| inst.world_aabb(&world.models[inst.model_id]).into())
            .collect();

        let (bvh_nodes, bvh_indices) = bvh::build(&aabbs);

        // Pack GPU instance data in BVH leaf order
        let gpu_instances: Vec<VoxelInstanceGpu> = bvh_indices
            .iter()
            .map(|&orig_idx| {
                let inst = instances[orig_idx as usize];
                let model = &world.models[inst.model_id];
                let offset = grid_offsets[inst.model_id];
                VoxelInstanceGpu::from_instance(inst, model, offset)
            })
            .collect();

        let instance_count = gpu_instances.len() as u32;

        // Upload BVH nodes
        let bvh_buf = if bvh_nodes.is_empty() {
            None
        } else {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bvh_nodes"),
                size: (bvh_nodes.len() * size_of::<bvh::BvhNode>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            {
                let mut view = buf
                    .slice(..)
                    .get_mapped_range_mut()
                    .expect("failed to map BVH buffer");
                view.copy_from_slice(bytemuck::cast_slice(&bvh_nodes));
            }
            buf.unmap();
            Some(buf)
        };

        // Upload packed grids
        let grid_buf = if packed_grids.is_empty() {
            None
        } else {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("packed_object_grids"),
                size: (packed_grids.len() * size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            {
                let mut view = buf
                    .slice(..)
                    .get_mapped_range_mut()
                    .expect("failed to map packed grids buffer");
                view.copy_from_slice(bytemuck::cast_slice(&packed_grids));
            }
            buf.unmap();
            Some(buf)
        };

        // Upload instances
        let instance_buf = {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("object_instances"),
                size: (gpu_instances.len() * size_of::<VoxelInstanceGpu>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: true,
            });
            {
                let mut view = buf
                    .slice(..)
                    .get_mapped_range_mut()
                    .expect("failed to map instance buffer");
                view.copy_from_slice(bytemuck::cast_slice(&gpu_instances));
            }
            buf.unmap();
            Some(buf)
        };

        log::info!(
            "world: {} instances, {} models, {} grid cells, {} BVH nodes",
            instance_count,
            world.models.len(),
            packed_grids.len(),
            bvh_nodes.len(),
        );

        Self {
            instance_buf,
            grid_buf,
            bvh_buf,
            instance_count,
            generation: 0,
        }
    }
}

/// The engine's central entity container. Pure CPU data — no GPU dependency.
///
/// Owns instanced voxel objects (models + instances). The
/// [`Engine`](crate::engine::Engine) handles extraction to GPU buffers
/// automatically during [`begin_frame`](crate::engine::Engine::begin_frame).
pub struct World {
    models: Vec<VoxelModel>,
    instances: slotmap::SlotMap<InstanceKey, VoxelInstance>,
    dirty: bool,
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
            models: Vec::new(),
            instances: slotmap::SlotMap::with_key(),
            dirty: false,
        }
    }

    /// Adds a model and returns its index.
    pub fn add_model(&mut self, model: VoxelModel) -> usize {
        let id = self.models.len();
        self.models.push(model);
        self.dirty = true;
        id
    }

    /// Adds an instance and returns its stable handle.
    pub fn add_instance(&mut self, instance: VoxelInstance) -> InstanceKey {
        let key = self.instances.insert(instance);
        self.dirty = true;
        key
    }

    /// Removes an instance by key. Returns the instance if it existed.
    pub fn remove_instance(&mut self, key: InstanceKey) -> Option<VoxelInstance> {
        let removed = self.instances.remove(key);
        if removed.is_some() {
            self.dirty = true;
        }
        removed
    }

    /// Read access to an instance.
    #[must_use]
    pub fn get(&self, key: InstanceKey) -> Option<&VoxelInstance> {
        self.instances.get(key)
    }

    /// Mutable access to an instance. Marks the world dirty.
    pub fn get_mut(&mut self, key: InstanceKey) -> Option<&mut VoxelInstance> {
        let inst = self.instances.get_mut(key);
        if inst.is_some() {
            self.dirty = true;
        }
        inst
    }

    /// Iterator over all instances.
    pub fn instances(&self) -> impl Iterator<Item = (InstanceKey, &VoxelInstance)> {
        self.instances.iter()
    }

    /// Access to the model list.
    #[must_use]
    pub fn models(&self) -> &[VoxelModel] {
        &self.models
    }

    /// Number of live instances.
    #[must_use]
    pub fn instance_count(&self) -> u32 {
        self.instances.len() as u32
    }

    /// Whether the world has been mutated since the last extraction.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clears the dirty flag. Called by Engine after extraction.
    pub(crate) fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}
