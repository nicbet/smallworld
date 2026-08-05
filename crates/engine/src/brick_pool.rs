//! Pooled GPU allocator for 16³ voxel bricks with per-brick material palettes.

/// Voxels packed per u32 (8-bit palette indices, 4 per word).
const VOXELS_PER_WORD: u32 = 4;

/// u32 words per brick in the voxel buffer.
pub const WORDS_PER_BRICK: u32 = BRICK_VOLUME / VOXELS_PER_WORD;

/// Maximum palette entries per brick (8-bit index range).
pub const PALETTE_ENTRIES: u32 = 256;

/// Voxels along one brick edge.
pub const BRICK_EDGE: u32 = 16;

/// Total voxels in one brick.
pub const BRICK_VOLUME: u32 = BRICK_EDGE * BRICK_EDGE * BRICK_EDGE;

/// Opaque handle to a live brick in the pool.
///
/// The `slot` field is the GPU buffer index; `generation` is a CPU-side guard
/// that detects use-after-free. For GPU bind groups, use [`gpu_index()`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BrickHandle {
    slot: u32,
    generation: u32,
}

impl BrickHandle {
    /// Sentinel for empty index entries.
    pub const NONE: Self = Self {
        slot: u32::MAX,
        generation: 0,
    };

    /// The raw slot index for GPU buffer addressing.
    #[must_use]
    pub fn gpu_index(self) -> u32 {
        self.slot
    }

    /// True if this is the [`NONE`](Self::NONE) sentinel.
    #[must_use]
    pub fn is_none(self) -> bool {
        self.slot == u32::MAX
    }
}

/// Pooled GPU allocator for 16³ voxel bricks.
///
/// Owns two GPU storage buffers (voxel data + material palettes) and manages
/// slot allocation via a free-list stack with generation-based validation.
///
/// Auxiliary per-brick channels (e.g. fluid level/flow) can be added later by
/// allocating additional buffers of `capacity × channel_size` and indexing with
/// [`BrickHandle::gpu_index()`].
pub struct BrickPool {
    voxel_buf: wgpu::Buffer,
    palette_buf: wgpu::Buffer,
    generations: Vec<u32>,
    free_list: Vec<u32>,
    capacity: u32,
    live_count: u32,
}

impl BrickPool {
    /// Creates a pool with room for `capacity` bricks.
    ///
    /// Pre-allocates two GPU storage buffers:
    /// - Voxel buffer: `capacity × 4 KB` (1024 u32s per brick)
    /// - Palette buffer: `capacity × 1 KB` (256 u32s per brick)
    pub fn new(device: &wgpu::Device, capacity: u32) -> Self {
        let voxel_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brick_voxels"),
            size: u64::from(capacity) * u64::from(WORDS_PER_BRICK) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let palette_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brick_palettes"),
            size: u64::from(capacity) * u64::from(PALETTE_ENTRIES) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let generations = vec![0u32; capacity as usize];
        let free_list: Vec<u32> = (0..capacity).rev().collect();

        log::info!(
            "brick pool: {capacity} slots, {:.1} MB voxels + {:.1} MB palettes",
            (capacity as f64 * WORDS_PER_BRICK as f64 * 4.0) / (1024.0 * 1024.0),
            (capacity as f64 * PALETTE_ENTRIES as f64 * 4.0) / (1024.0 * 1024.0),
        );

        Self {
            voxel_buf,
            palette_buf,
            generations,
            free_list,
            capacity,
            live_count: 0,
        }
    }

    /// Allocates a brick slot, returning its handle. Returns `None` if the pool
    /// is exhausted.
    pub fn alloc(&mut self) -> Option<BrickHandle> {
        let slot = self.free_list.pop()?;
        self.live_count += 1;
        Some(BrickHandle {
            slot,
            generation: self.generations[slot as usize],
        })
    }

    /// Returns a brick slot to the free list for reuse.
    ///
    /// # Panics
    ///
    /// Debug-asserts that the handle's generation matches the slot's current
    /// generation (detects use-after-free).
    pub fn free(&mut self, handle: BrickHandle) {
        debug_assert!(
            !handle.is_none(),
            "cannot free BrickHandle::NONE"
        );
        let slot = handle.slot as usize;
        debug_assert!(
            slot < self.capacity as usize,
            "brick handle slot {slot} out of range (capacity {})",
            self.capacity
        );
        debug_assert_eq!(
            handle.generation, self.generations[slot],
            "stale brick handle (gen {} vs current {})",
            handle.generation, self.generations[slot]
        );

        self.generations[slot] = self.generations[slot].wrapping_add(1);
        self.free_list.push(handle.slot);
        self.live_count -= 1;
    }

    /// Whether the handle refers to a currently live brick.
    #[must_use]
    pub fn is_valid(&self, handle: BrickHandle) -> bool {
        if handle.is_none() {
            return false;
        }
        let slot = handle.slot as usize;
        slot < self.capacity as usize && handle.generation == self.generations[slot]
    }

    /// Uploads voxel data (4096 × u8) for the given brick, packing four u8
    /// values per u32 word.
    pub fn write_voxels(&self, queue: &wgpu::Queue, handle: BrickHandle, data: &[u8; BRICK_VOLUME as usize]) {
        debug_assert!(self.is_valid(handle), "writing voxels to an invalid handle");

        let mut packed = [0u32; WORDS_PER_BRICK as usize];
        for (i, chunk) in data.chunks_exact(VOXELS_PER_WORD as usize).enumerate() {
            packed[i] = u32::from(chunk[0])
                | (u32::from(chunk[1]) << 8)
                | (u32::from(chunk[2]) << 16)
                | (u32::from(chunk[3]) << 24);
        }

        let offset = u64::from(handle.slot) * u64::from(WORDS_PER_BRICK) * 4;
        queue.write_buffer(&self.voxel_buf, offset, bytemuck::cast_slice(&packed));
    }

    /// Uploads palette entries (RGBA, 4 bytes each) for the given brick.
    ///
    /// `entries` may have fewer than 256 elements; indices beyond the slice
    /// length are unspecified on the GPU.
    pub fn write_palette(&self, queue: &wgpu::Queue, handle: BrickHandle, entries: &[[u8; 4]]) {
        debug_assert!(self.is_valid(handle), "writing palette to an invalid handle");
        debug_assert!(
            entries.len() <= PALETTE_ENTRIES as usize,
            "palette has {} entries, max is {PALETTE_ENTRIES}",
            entries.len()
        );

        let packed: Vec<u32> = entries
            .iter()
            .map(|&[r, g, b, a]| {
                u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16) | (u32::from(a) << 24)
            })
            .collect();

        let offset = u64::from(handle.slot) * u64::from(PALETTE_ENTRIES) * 4;
        queue.write_buffer(&self.palette_buf, offset, bytemuck::cast_slice(&packed));
    }

    /// The GPU voxel storage buffer, for shader bind group creation.
    #[must_use]
    pub fn voxel_buffer(&self) -> &wgpu::Buffer {
        &self.voxel_buf
    }

    /// The GPU palette storage buffer, for shader bind group creation.
    #[must_use]
    pub fn palette_buffer(&self) -> &wgpu::Buffer {
        &self.palette_buf
    }

    /// Maximum number of bricks this pool can hold.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Number of currently allocated (live) bricks.
    #[must_use]
    pub fn live_count(&self) -> u32 {
        self.live_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_of(h: BrickHandle) -> u32 {
        h.gpu_index()
    }

    #[test]
    fn alloc_returns_sequential_slots() {
        let mut pool = test_pool(4);
        let a = pool.alloc().unwrap();
        let b = pool.alloc().unwrap();
        assert_eq!(slot_of(a), 0);
        assert_eq!(slot_of(b), 1);
        assert_eq!(pool.live_count(), 2);
    }

    #[test]
    fn free_and_realloc_reuses_slot() {
        let mut pool = test_pool(2);
        let a = pool.alloc().unwrap();
        let b = pool.alloc().unwrap();
        pool.free(a);
        let c = pool.alloc().unwrap();
        assert_eq!(slot_of(c), slot_of(a));
        assert_ne!(c.generation, a.generation);
        assert_eq!(pool.live_count(), 2);
        // b is still valid
        assert!(pool.is_valid(b));
    }

    #[test]
    fn exhaustion_returns_none() {
        let mut pool = test_pool(1);
        assert!(pool.alloc().is_some());
        assert!(pool.alloc().is_none());
    }

    #[test]
    fn stale_handle_is_invalid() {
        let mut pool = test_pool(2);
        let a = pool.alloc().unwrap();
        pool.free(a);
        assert!(!pool.is_valid(a));
    }

    #[test]
    #[should_panic(expected = "stale brick handle")]
    fn double_free_panics() {
        let mut pool = test_pool(2);
        let a = pool.alloc().unwrap();
        pool.free(a);
        pool.free(a);
    }

    #[test]
    fn none_sentinel() {
        let pool = test_pool(1);
        assert!(BrickHandle::NONE.is_none());
        assert!(!pool.is_valid(BrickHandle::NONE));
    }

    #[test]
    fn generation_wraps() {
        let mut pool = test_pool(1);
        for _ in 0..1000 {
            let h = pool.alloc().unwrap();
            pool.free(h);
        }
        let h = pool.alloc().unwrap();
        assert_eq!(h.generation, 1000);
        assert!(pool.is_valid(h));
    }

    /// Creates a pool without a real GPU device (tests only exercise the
    /// allocator logic, not GPU uploads).
    fn test_pool(capacity: u32) -> BrickPool {
        use crate::gpu::GpuContext;
        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));
        BrickPool::new(&ctx.device, capacity)
    }
}
