//! Persistent coarse mip grid: retains mip levels 2–4 for every loaded brick,
//! independent of brick pool eviction. Distant terrain renders at low resolution
//! instead of vanishing.

use crate::mip::MIP_WORDS_PER_BRICK;

/// u32 words stored per cell: levels 2 (4³=64) + 3 (2³=8) + 4 (1³=1).
pub const COARSE_MIP_WORDS: u32 = 64 + 8 + 1;

/// Offsets into the full 585-word mip array where each level starts.
const LEVEL2_SRC_OFFSET: usize = 512; // after level 1 (8³)
const LEVEL3_SRC_OFFSET: usize = 576; // after level 2 (4³)
const LEVEL4_SRC_OFFSET: usize = 584; // after level 3 (2³)

/// Grid-parallel buffer storing coarse mip data (levels 2–4) per cell.
///
/// Written when a brick is first loaded, never cleared on eviction. The shader
/// falls back to this data when the full brick has been evicted from the pool.
pub struct CoarseMipGrid {
    buffer: wgpu::Buffer,
    data: Vec<u32>,
    dims: [u32; 3],
}

impl CoarseMipGrid {
    /// Creates a coarse mip grid matching the given dimensions.
    pub fn new(device: &wgpu::Device, dims: [u32; 3]) -> Self {
        let total = (dims[0] * dims[1] * dims[2]) as usize;
        let words = total * COARSE_MIP_WORDS as usize;
        let data = vec![0u32; words];

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coarse_mip_grid"),
            size: (words * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let size_mb = (words * 4) as f64 / (1024.0 * 1024.0);
        log::info!(
            "coarse mip grid: {}×{}×{} ({total} cells, {size_mb:.1} MB)",
            dims[0],
            dims[1],
            dims[2],
        );

        Self { buffer, data, dims }
    }

    /// Writes coarse mip data (levels 2–4) for a grid cell from a full
    /// 585-word mip array.
    pub fn write_cell(&mut self, pos: [u32; 3], full_mips: &[u32; MIP_WORDS_PER_BRICK as usize]) {
        let flat = self.flat_index(pos);
        let dst_base = flat * COARSE_MIP_WORDS as usize;

        self.data[dst_base..dst_base + 64]
            .copy_from_slice(&full_mips[LEVEL2_SRC_OFFSET..LEVEL2_SRC_OFFSET + 64]);
        self.data[dst_base + 64..dst_base + 72]
            .copy_from_slice(&full_mips[LEVEL3_SRC_OFFSET..LEVEL3_SRC_OFFSET + 8]);
        self.data[dst_base + 72] = full_mips[LEVEL4_SRC_OFFSET];
    }

    /// Uploads the full CPU mirror to the GPU buffer.
    pub fn upload(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.data));
    }

    /// Uploads only the region for a single cell.
    pub fn upload_cell(&self, queue: &wgpu::Queue, pos: [u32; 3]) {
        let flat = self.flat_index(pos);
        let word_offset = flat * COARSE_MIP_WORDS as usize;
        let byte_offset = (word_offset * 4) as u64;
        let slice = &self.data[word_offset..word_offset + COARSE_MIP_WORDS as usize];
        queue.write_buffer(&self.buffer, byte_offset, bytemuck::cast_slice(slice));
    }

    /// The GPU storage buffer, for shader bind group creation.
    #[must_use]
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    fn flat_index(&self, pos: [u32; 3]) -> usize {
        debug_assert!(
            pos[0] < self.dims[0] && pos[1] < self.dims[1] && pos[2] < self.dims[2],
            "coarse mip grid position {:?} out of bounds {:?}",
            pos,
            self.dims
        );
        (pos[0] + self.dims[0] * (pos[1] + self.dims[1] * pos[2])) as usize
    }
}
