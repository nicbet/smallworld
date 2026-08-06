//! Instanced voxel objects: models (shared voxel data) and instances (transforms).

use crate::brick_pool::{BRICK_EDGE, BRICK_VOLUME, BrickHandle, BrickPool};
use glam::{Mat4, Quat, Vec3};

/// Shared voxel data for an object type (tree, rock, prop).
///
/// Stores a local brick grid whose handles index into the global [`BrickPool`].
/// Multiple [`VoxelInstance`]s can reference the same model.
pub struct VoxelModel {
    grid: Vec<u32>,
    dims: [u32; 3],
    voxel_scale: f32,
}

impl VoxelModel {
    /// Creates an empty model with the given grid dimensions and voxel scale.
    #[must_use]
    pub fn new(dims: [u32; 3], voxel_scale: f32) -> Self {
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        Self {
            grid: vec![u32::MAX; total],
            dims,
            voxel_scale,
        }
    }

    /// Sets the brick handle at local grid position.
    pub fn set(&mut self, pos: [u32; 3], handle: BrickHandle) {
        let idx = self.flat_index(pos);
        self.grid[idx] = handle.gpu_index();
    }

    /// Reads the raw handle value at local grid position.
    #[must_use]
    pub fn get(&self, pos: [u32; 3]) -> u32 {
        self.grid[self.flat_index(pos)]
    }

    /// World-space extent of the model (dims × brick_edge × voxel_scale).
    #[must_use]
    pub fn world_extent(&self) -> Vec3 {
        let brick_size = BRICK_EDGE as f32 * self.voxel_scale;
        Vec3::new(
            self.dims[0] as f32 * brick_size,
            self.dims[1] as f32 * brick_size,
            self.dims[2] as f32 * brick_size,
        )
    }

    /// The flat grid data for GPU upload.
    #[must_use]
    pub fn grid_data(&self) -> &[u32] {
        &self.grid
    }

    /// Grid dimensions in bricks.
    #[must_use]
    pub fn dims(&self) -> [u32; 3] {
        self.dims
    }

    /// Voxel scale in metres.
    #[must_use]
    pub fn voxel_scale(&self) -> f32 {
        self.voxel_scale
    }

    /// Number of allocated (non-empty) bricks.
    #[must_use]
    pub fn brick_count(&self) -> u32 {
        self.grid.iter().filter(|&&v| v != u32::MAX).count() as u32
    }

    fn flat_index(&self, pos: [u32; 3]) -> usize {
        debug_assert!(
            pos[0] < self.dims[0] && pos[1] < self.dims[1] && pos[2] < self.dims[2],
            "model grid position {:?} out of bounds {:?}",
            pos,
            self.dims
        );
        (pos[0] + self.dims[0] * (pos[1] + self.dims[1] * pos[2])) as usize
    }

    /// Generates voxel data for one brick using a closure, allocates from the
    /// pool, and sets the grid entry. Returns true if the brick had content.
    pub fn fill_brick(
        &mut self,
        pos: [u32; 3],
        pool: &mut BrickPool,
        queue: &wgpu::Queue,
        voxels: &[u8; BRICK_VOLUME as usize],
        palette: &[[u8; 4]],
    ) -> bool {
        let has_content = voxels.iter().any(|&v| v != 0);
        if !has_content {
            return false;
        }
        let handle = pool.alloc().expect("brick pool exhausted");
        pool.write_voxels(queue, handle, voxels);
        pool.write_palette(queue, handle, palette);
        let mips = crate::mip::compute_brick_mips(voxels, palette);
        pool.write_mips(queue, handle, &mips);
        self.set(pos, handle);
        true
    }
}

/// CPU-side instance: references a model and places it in the world.
pub struct VoxelInstance {
    /// Index into the scene's model list.
    pub model_id: usize,
    /// World-space position (translation applied after rotation).
    pub position: Vec3,
    /// Rotation quaternion.
    pub rotation: Quat,
}

impl VoxelInstance {
    /// Computes the object→world transform matrix.
    #[must_use]
    pub fn transform(&self, model: &VoxelModel) -> Mat4 {
        let extent = model.world_extent();
        let center_offset = extent * -0.5;
        Mat4::from_translation(self.position)
            * Mat4::from_quat(self.rotation)
            * Mat4::from_translation(center_offset)
    }

    /// Computes the world-space AABB for this instance.
    #[must_use]
    pub fn world_aabb(&self, model: &VoxelModel) -> (Vec3, Vec3) {
        let transform = self.transform(model);
        let extent = model.world_extent();
        let corners = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(extent.x, 0.0, 0.0),
            Vec3::new(0.0, extent.y, 0.0),
            Vec3::new(extent.x, extent.y, 0.0),
            Vec3::new(0.0, 0.0, extent.z),
            Vec3::new(extent.x, 0.0, extent.z),
            Vec3::new(0.0, extent.y, extent.z),
            Vec3::new(extent.x, extent.y, extent.z),
        ];
        let mut aabb_min = Vec3::splat(f32::MAX);
        let mut aabb_max = Vec3::splat(f32::MIN);
        for c in &corners {
            let w = transform.transform_point3(*c);
            aabb_min = aabb_min.min(w);
            aabb_max = aabb_max.max(w);
        }
        (aabb_min, aabb_max)
    }
}

/// GPU-side instance data, packed for shader consumption.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VoxelInstanceGpu {
    /// Object → world transform.
    pub transform: [f32; 16],
    /// World → object transform.
    pub inv_transform: [f32; 16],
    /// World AABB min; w = voxel_scale.
    pub aabb_min: [f32; 4],
    /// World AABB max; w = grid_offset (u32 as f32).
    pub aabb_max: [f32; 4],
    /// Grid dims; w = total grid cells.
    pub grid_dims: [u32; 4],
}

impl VoxelInstanceGpu {
    /// Builds GPU instance data from a CPU instance + model + grid offset.
    #[must_use]
    pub fn from_instance(inst: &VoxelInstance, model: &VoxelModel, grid_offset: u32) -> Self {
        let transform = inst.transform(model);
        let inv_transform = transform.inverse();
        let (aabb_min, aabb_max) = inst.world_aabb(model);
        let dims = model.dims();
        Self {
            transform: transform.to_cols_array(),
            inv_transform: inv_transform.to_cols_array(),
            aabb_min: [aabb_min.x, aabb_min.y, aabb_min.z, model.voxel_scale()],
            aabb_max: [
                aabb_max.x,
                aabb_max.y,
                aabb_max.z,
                f32::from_bits(grid_offset),
            ],
            grid_dims: [dims[0], dims[1], dims[2], dims[0] * dims[1] * dims[2]],
        }
    }
}
