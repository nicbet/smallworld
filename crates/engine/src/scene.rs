//! Scene: terrain + instanced voxel objects with packed GPU buffers.

use crate::bvh;
use crate::voxel_object::{VoxelInstance, VoxelInstanceGpu, VoxelModel};

/// Holds all instanced voxel objects and their packed GPU data.
pub struct Scene {
    models: Vec<VoxelModel>,
    instances: Vec<VoxelInstance>,
    packed_grids_buf: Option<wgpu::Buffer>,
    instance_buf: Option<wgpu::Buffer>,
    bvh_buf: Option<wgpu::Buffer>,
    instance_count: u32,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    /// Creates an empty scene.
    #[must_use]
    pub fn new() -> Self {
        Self {
            models: Vec::new(),
            instances: Vec::new(),
            packed_grids_buf: None,
            instance_buf: None,
            bvh_buf: None,
            instance_count: 0,
        }
    }

    /// Adds a model and returns its index.
    pub fn add_model(&mut self, model: VoxelModel) -> usize {
        let id = self.models.len();
        self.models.push(model);
        id
    }

    /// Adds an instance.
    pub fn add_instance(&mut self, instance: VoxelInstance) {
        self.instances.push(instance);
    }

    /// Packs all model grids and instance data into GPU buffers.
    pub fn upload(&mut self, device: &wgpu::Device) {
        if self.instances.is_empty() {
            self.instance_count = 0;
            return;
        }

        // Pack model grids: compute offsets, concatenate
        let mut grid_offsets: Vec<u32> = Vec::with_capacity(self.models.len());
        let mut packed_grids: Vec<u32> = Vec::new();
        for model in &self.models {
            grid_offsets.push(packed_grids.len() as u32);
            packed_grids.extend_from_slice(model.grid_data());
        }

        // Build AABBs for BVH
        let aabbs: Vec<(glam::Vec3, glam::Vec3)> = self
            .instances
            .iter()
            .map(|inst| inst.world_aabb(&self.models[inst.model_id]))
            .collect();

        // Build BVH (reorders indices)
        let (bvh_nodes, bvh_indices) = bvh::build(&aabbs);

        // Build GPU instance data in BVH leaf order
        let gpu_instances: Vec<VoxelInstanceGpu> = bvh_indices
            .iter()
            .map(|&orig_idx| {
                let inst = &self.instances[orig_idx as usize];
                let model = &self.models[inst.model_id];
                let offset = grid_offsets[inst.model_id];
                VoxelInstanceGpu::from_instance(inst, model, offset)
            })
            .collect();

        self.instance_count = gpu_instances.len() as u32;

        // Upload BVH nodes
        if !bvh_nodes.is_empty() {
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
            self.bvh_buf = Some(buf);
        }

        // Upload packed grids
        if !packed_grids.is_empty() {
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
            self.packed_grids_buf = Some(buf);
        }

        // Upload instances
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
        self.instance_buf = Some(buf);

        log::info!(
            "scene: {} instances, {} models, {} grid cells, {} BVH nodes",
            self.instance_count,
            self.models.len(),
            packed_grids.len(),
            bvh_nodes.len(),
        );
    }

    /// The packed object grid buffer, for shader bind groups.
    #[must_use]
    pub fn grid_buffer(&self) -> Option<&wgpu::Buffer> {
        self.packed_grids_buf.as_ref()
    }

    /// The BVH node buffer, for shader bind groups.
    #[must_use]
    pub fn bvh_buffer(&self) -> Option<&wgpu::Buffer> {
        self.bvh_buf.as_ref()
    }

    /// The instance data buffer, for shader bind groups.
    #[must_use]
    pub fn instance_buffer(&self) -> Option<&wgpu::Buffer> {
        self.instance_buf.as_ref()
    }

    /// Number of instances uploaded.
    #[must_use]
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }

    /// Access to the model list.
    #[must_use]
    pub fn models(&self) -> &[VoxelModel] {
        &self.models
    }

    /// Access to the instance list.
    #[must_use]
    pub fn instances(&self) -> &[VoxelInstance] {
        &self.instances
    }
}
