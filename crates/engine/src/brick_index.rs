//! Flat 3D grid mapping world-space brick coordinates to brick pool handles.

use crate::brick_pool::{BRICK_EDGE, BrickHandle, VOXEL_SCALE};
use glam::Vec3;

/// Flat 3D grid of brick handles, backed by a GPU storage buffer.
///
/// Each cell holds a `u32` slot index into the brick pool (`u32::MAX` = empty).
/// A CPU-side mirror supports construction and queries; call [`upload`](Self::upload)
/// to push the mirror to the GPU.
pub struct BrickIndex {
    buffer: wgpu::Buffer,
    data: Vec<u32>,
    dims: [u32; 3],
    world_min: Vec3,
}

/// World-space edge length of one brick (16 voxels × 0.1 m).
pub const BRICK_SIZE: f32 = BRICK_EDGE as f32 * VOXEL_SCALE;

impl BrickIndex {
    /// Creates an index grid of the given dimensions, centered at `world_min`.
    pub fn new(device: &wgpu::Device, dims: [u32; 3], world_min: Vec3) -> Self {
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        let data = vec![u32::MAX; total];

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brick_index"),
            size: (total * size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped at creation")
            .copy_from_slice(bytemuck::cast_slice(&data));
        buffer.unmap();

        log::info!(
            "brick index: {}×{}×{} ({total} cells, {:.1} KB)",
            dims[0],
            dims[1],
            dims[2],
            (total * 4) as f64 / 1024.0,
        );

        Self {
            buffer,
            data,
            dims,
            world_min,
        }
    }

    /// Sets the brick handle at grid position `[x, y, z]`.
    pub fn set(&mut self, pos: [u32; 3], handle: BrickHandle) {
        let idx = self.flat_index(pos);
        self.data[idx] = handle.gpu_index();
    }

    /// Reads the raw handle value at grid position `[x, y, z]`.
    #[must_use]
    pub fn get(&self, pos: [u32; 3]) -> u32 {
        self.data[self.flat_index(pos)]
    }

    /// Uploads the full CPU mirror to the GPU buffer.
    pub fn upload(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.data));
    }

    /// The GPU storage buffer, for shader bind group creation.
    #[must_use]
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Grid dimensions in bricks `[x, y, z]`.
    #[must_use]
    pub fn dims(&self) -> [u32; 3] {
        self.dims
    }

    /// World-space minimum corner of the grid.
    #[must_use]
    pub fn world_min(&self) -> Vec3 {
        self.world_min
    }

    /// World-space edge length of one brick.
    #[must_use]
    pub fn brick_size(&self) -> f32 {
        BRICK_SIZE
    }

    /// Number of non-empty cells in the CPU mirror.
    #[must_use]
    pub fn occupied_count(&self) -> u32 {
        self.data.iter().filter(|&&v| v != u32::MAX).count() as u32
    }

    /// Clears a grid cell back to empty (`u32::MAX`).
    pub fn clear_cell(&mut self, pos: [u32; 3]) {
        let idx = self.flat_index(pos);
        self.data[idx] = u32::MAX;
    }

    fn flat_index(&self, pos: [u32; 3]) -> usize {
        debug_assert!(
            pos[0] < self.dims[0] && pos[1] < self.dims[1] && pos[2] < self.dims[2],
            "grid position {:?} out of bounds {:?}",
            pos,
            self.dims
        );
        (pos[0] + self.dims[0] * (pos[1] + self.dims[1] * pos[2])) as usize
    }
}
