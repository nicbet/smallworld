//! Sparse Voxel Octree — hierarchical spatial index with seamless LOD.
//!
//! Each interior node stores the averaged color of its descendants, so any
//! node can be rendered as a single colored voxel at the appropriate size.
//! Leaf nodes reference bricks in the [`BrickPool`](crate::brick_pool::BrickPool)
//! for fine-grained voxel detail.

use crate::brick_pool::BrickHandle;
use glam::Vec3;

/// GPU-side octree node. 16 bytes, tightly packed.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SvoNode {
    /// Index into the node array where this node's 8 children start.
    /// 0 = no children (leaf or empty).
    pub children: u32,
    /// Brick handle (`u32::MAX` = no brick attached).
    pub brick: u32,
    /// Packed RGBA — averaged color of all descendant solid voxels.
    pub color: u32,
    /// Bits 0–7: valid child mask (bit i = child i exists).
    /// Bit 8: has brick data (leaf with voxel detail).
    pub flags: u32,
}

impl SvoNode {
    const EMPTY: Self = Self {
        children: 0,
        brick: u32::MAX,
        color: 0,
        flags: 0,
    };

    fn child_mask(&self) -> u8 {
        (self.flags & 0xFF) as u8
    }

    fn has_children(&self) -> bool {
        self.children != 0 && self.child_mask() != 0
    }
}

const CHILD_BLOCK: u32 = 8;

/// Sparse Voxel Octree backed by a GPU storage buffer.
pub struct Svo {
    nodes: Vec<SvoNode>,
    free_blocks: Vec<u32>,
    root: u32,
    /// World-space origin (minimum corner).
    world_min: Vec3,
    /// World edge length (cube, power-of-2 aligned).
    world_size: f32,
    buffer: wgpu::Buffer,
    capacity: u32,
}

impl Svo {
    /// Creates an SVO with room for `capacity` nodes.
    pub fn new(device: &wgpu::Device, capacity: u32, world_min: Vec3, world_size: f32) -> Self {
        let mut nodes = vec![SvoNode::EMPTY; capacity as usize];

        let root = 0u32;
        nodes[root as usize] = SvoNode::EMPTY;

        let mut free_blocks: Vec<u32> = Vec::new();
        let mut idx = 1u32;
        while idx + CHILD_BLOCK <= capacity {
            free_blocks.push(idx);
            idx += CHILD_BLOCK;
        }

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("svo_nodes"),
            size: u64::from(capacity) * size_of::<SvoNode>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let size_mb = capacity as f64 * size_of::<SvoNode>() as f64 / (1024.0 * 1024.0);
        log::info!("SVO: {capacity} nodes ({size_mb:.1} MB), world_size={world_size:.1}m");

        Self {
            nodes,
            free_blocks,
            root,
            world_min,
            world_size,
            buffer,
            capacity,
        }
    }

    /// Inserts a brick whose minimum corner is at `world_pos`, creating
    /// intermediate nodes as needed. `leaf_size` is the world-space edge
    /// length of the leaf node (typically `BRICK_EDGE * VOXEL_SCALE`).
    ///
    /// The position is snapped to the nearest leaf-lattice point and the
    /// descent runs on integer cell coordinates. Min corners sit exactly on
    /// octree split planes, so classifying them with accumulated f32 centers
    /// mis-sorts bricks whose rounding error crosses the plane — one-ulp
    /// disagreements turn into entire missing brick planes.
    pub fn insert_brick(
        &mut self,
        world_pos: Vec3,
        leaf_size: f32,
        handle: BrickHandle,
        color: [u8; 4],
    ) {
        let depth = (self.world_size / leaf_size).log2().round() as u32;
        let cells = 1i64 << depth;
        let scale = f64::from(cells as u32) / f64::from(self.world_size);
        let rel = world_pos - self.world_min;
        let quant = |v: f32| ((f64::from(v) * scale).round() as i64).clamp(0, cells - 1) as u32;
        let (cx, cy, cz) = (quant(rel.x), quant(rel.y), quant(rel.z));

        let packed_color = pack_rgba(color);
        let mut node_idx = self.root;

        for level in (0..depth).rev() {
            let octant =
                ((cx >> level) & 1) | (((cy >> level) & 1) << 1) | (((cz >> level) & 1) << 2);

            if self.nodes[node_idx as usize].children == 0 {
                let block = self.alloc_children();
                self.nodes[node_idx as usize].children = block;
            }

            self.nodes[node_idx as usize].flags |= 1u32 << octant;
            node_idx = self.nodes[node_idx as usize].children + octant;
        }

        self.nodes[node_idx as usize].brick = handle.gpu_index();
        self.nodes[node_idx as usize].color = packed_color;
        self.nodes[node_idx as usize].flags |= 1 << 8;
    }

    /// Recomputes interior node colors bottom-up from leaves.
    pub fn update_colors(&mut self) {
        self.update_colors_recursive(self.root);
    }

    #[allow(clippy::manual_checked_ops)]
    fn update_colors_recursive(&mut self, idx: u32) -> (u32, u32, u32, u32, u32) {
        let node = self.nodes[idx as usize];
        if !node.has_children() {
            let r = node.color & 0xFF;
            let g = (node.color >> 8) & 0xFF;
            let b = (node.color >> 16) & 0xFF;
            let a = (node.color >> 24) & 0xFF;
            return (r, g, b, a, if a > 0 { 1 } else { 0 });
        }

        let children_base = node.children;
        let mask = node.child_mask();
        let mut r_sum = 0u32;
        let mut g_sum = 0u32;
        let mut b_sum = 0u32;
        let mut a_sum = 0u32;
        let mut count = 0u32;

        for i in 0..8u32 {
            if mask & (1 << i) != 0 {
                let (cr, cg, cb, ca, cc) = self.update_colors_recursive(children_base + i);
                if cc > 0 {
                    r_sum += cr * cc;
                    g_sum += cg * cc;
                    b_sum += cb * cc;
                    a_sum += ca * cc;
                    count += cc;
                }
            }
        }

        if count > 0 {
            let color = pack_rgba([
                (r_sum / count) as u8,
                (g_sum / count) as u8,
                (b_sum / count) as u8,
                (a_sum / count) as u8,
            ]);
            self.nodes[idx as usize].color = color;
        }

        (
            r_sum.checked_div(count).unwrap_or(0),
            g_sum.checked_div(count).unwrap_or(0),
            b_sum.checked_div(count).unwrap_or(0),
            a_sum.checked_div(count).unwrap_or(0),
            count,
        )
    }

    /// Uploads the CPU node array to the GPU buffer.
    pub fn upload(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.nodes));
    }

    /// The GPU storage buffer for shader binding.
    #[must_use]
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Root node index.
    #[must_use]
    pub fn root(&self) -> u32 {
        self.root
    }

    /// World-space minimum corner.
    #[must_use]
    pub fn world_min(&self) -> Vec3 {
        self.world_min
    }

    /// World cube edge length.
    #[must_use]
    pub fn world_size(&self) -> f32 {
        self.world_size
    }

    /// Number of allocated nodes.
    #[must_use]
    pub fn node_count(&self) -> u32 {
        self.capacity - self.free_blocks.len() as u32 * CHILD_BLOCK
    }

    fn alloc_children(&mut self) -> u32 {
        let block = self.free_blocks.pop().expect("SVO node pool exhausted");
        for i in 0..CHILD_BLOCK {
            self.nodes[(block + i) as usize] = SvoNode::EMPTY;
        }
        block
    }
}

fn pack_rgba(c: [u8; 4]) -> u32 {
    u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16) | (u32::from(c[3]) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_svo() -> Svo {
        use crate::gpu::GpuContext;
        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));
        Svo::new(&ctx.device, 1024, Vec3::ZERO, 16.0)
    }

    #[test]
    fn insert_and_find_leaf() {
        let mut svo = test_svo();
        let handle = BrickHandle::NONE;

        svo.insert_brick(Vec3::new(1.0, 1.0, 1.0), 1.0, handle, [200, 100, 50, 255]);
        svo.update_colors();

        let root = svo.nodes[svo.root as usize];
        assert!(root.has_children());

        let root_color = root.color;
        let r = root_color & 0xFF;
        let g = (root_color >> 8) & 0xFF;
        let b = (root_color >> 16) & 0xFF;
        assert_eq!(r, 200);
        assert_eq!(g, 100);
        assert_eq!(b, 50);
    }

    #[test]
    fn two_bricks_average_color() {
        let mut svo = test_svo();
        let handle = BrickHandle::NONE;

        svo.insert_brick(Vec3::new(1.0, 1.0, 1.0), 1.0, handle, [200, 0, 0, 255]);
        svo.insert_brick(Vec3::new(9.0, 1.0, 1.0), 1.0, handle, [0, 200, 0, 255]);
        svo.update_colors();

        let root_color = svo.nodes[svo.root as usize].color;
        let r = root_color & 0xFF;
        let g = (root_color >> 8) & 0xFF;
        assert_eq!(r, 100);
        assert_eq!(g, 100);
    }

    /// Regression test for grid-shaped chasms: brick min corners sit exactly
    /// on octree split planes, and f32-accumulated centers mis-sorted bricks
    /// at indices whose `g * BRICK_SIZE` rounding error crossed the plane
    /// (x/z 7, 14, 15, 19, 20, ... for a 32-cell world). Every misplaced
    /// brick overwrote its neighbor's leaf and left its own cell empty.
    /// Inserting the full 32×12×32 grid must yield exactly one distinct
    /// brick leaf per cell.
    #[test]
    fn min_corner_inserts_land_in_distinct_leaves() {
        use crate::gpu::GpuContext;
        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));

        // Mirror the Default preset's arithmetic exactly (main.rs / scenes.rs).
        let brick_size = 16.0_f32 * 0.1_f32;
        let dims = [32u32, 12, 32];
        let world_size = (32.0_f32 * 16.0_f32) * 0.1_f32;
        let half = Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32) * brick_size * 0.5;
        let world_min = -half;

        let mut svo = Svo::new(&ctx.device, 1_000_000, world_min, world_size);

        for gz in 0..dims[2] {
            for gy in 0..dims[1] {
                for gx in 0..dims[0] {
                    let pos = world_min + Vec3::new(gx as f32, gy as f32, gz as f32) * brick_size;
                    svo.insert_brick(pos, brick_size, BrickHandle::NONE, [100, 100, 100, 255]);
                }
            }
        }

        let brick_leaves = svo.nodes.iter().filter(|n| n.flags & (1 << 8) != 0).count() as u32;
        assert_eq!(
            brick_leaves,
            dims[0] * dims[1] * dims[2],
            "every brick must occupy its own leaf — overwrites mean min corners were mis-sorted"
        );
    }

    #[test]
    fn node_count_grows() {
        let mut svo = test_svo();
        let initial = svo.node_count();
        svo.insert_brick(Vec3::new(1.0, 1.0, 1.0), 1.0, BrickHandle::NONE, [255; 4]);
        assert!(svo.node_count() > initial);
    }
}
