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
    /// Node indices modified since the last upload; drained by
    /// [`upload_dirty`](Self::upload_dirty).
    dirty: Vec<u32>,
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
            dirty: Vec::new(),
        }
    }

    /// Snaps a world position to integer leaf-cell coordinates and returns
    /// them with the tree depth for that leaf size. Min corners sit exactly
    /// on octree split planes, so classification must round in f64 and then
    /// descend on integers — accumulated f32 center comparisons mis-sort
    /// bricks whose rounding error crosses a plane.
    fn quantize_cell(&self, world_pos: Vec3, leaf_size: f32) -> ([u32; 3], u32) {
        let depth = (self.world_size / leaf_size).log2().round() as u32;
        let cells = 1i64 << depth;
        let scale = f64::from(cells as u32) / f64::from(self.world_size);
        let rel = world_pos - self.world_min;
        let q = |v: f32| ((f64::from(v) * scale).round() as i64).clamp(0, cells - 1) as u32;
        ([q(rel.x), q(rel.y), q(rel.z)], depth)
    }

    fn octant_at(cell: [u32; 3], level: u32) -> u32 {
        ((cell[0] >> level) & 1) | (((cell[1] >> level) & 1) << 1) | (((cell[2] >> level) & 1) << 2)
    }

    /// Returns the child of `parent` at `octant`, allocating the child block
    /// and setting the valid bit if needed. This is the building block for
    /// direct tree construction (coarse worldgen builds subtrees top-down
    /// without re-descending from the root per node).
    pub fn alloc_child(&mut self, parent: u32, octant: u8) -> u32 {
        if self.nodes[parent as usize].children == 0 {
            let block = self.alloc_children();
            let p = self.nodes[parent as usize];
            // Subdividing a coarse solid leaf (colored, brickless, childless):
            // materialize all 8 children with the parent's color first, or the
            // octants not on the descent path turn from solid volume into air.
            if p.flags & (1 << 8) == 0 && (p.color >> 24) & 0xFF > 0 {
                for i in 0..CHILD_BLOCK {
                    self.nodes[(block + i) as usize].color = p.color;
                }
                self.nodes[parent as usize].flags |= 0xFF;
                self.dirty.push(parent);
            }
            self.nodes[parent as usize].children = block;
        }
        let bit = 1u32 << octant;
        if self.nodes[parent as usize].flags & bit == 0 {
            self.nodes[parent as usize].flags |= bit;
            self.dirty.push(parent);
        }
        self.nodes[parent as usize].children + u32::from(octant)
    }

    /// Sets a node's color directly (coarse construction path).
    pub fn set_color(&mut self, node_idx: u32, color: [u8; 4]) {
        self.nodes[node_idx as usize].color = pack_rgba(color);
        self.dirty.push(node_idx);
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
        let (cell, depth) = self.quantize_cell(world_pos, leaf_size);

        let mut node_idx = self.root;
        for level in (0..depth).rev() {
            let octant = Self::octant_at(cell, level) as u8;
            node_idx = self.alloc_child(node_idx, octant);
        }

        let node = &mut self.nodes[node_idx as usize];
        node.brick = handle.gpu_index();
        node.color = pack_rgba(color);
        node.flags |= 1 << 8;
        self.dirty.push(node_idx);

        self.refresh_path_colors(cell, depth);
    }

    /// Detaches the brick from the leaf at `world_pos`, keeping the leaf's
    /// averaged color as the LOD fallback. Called on pager eviction — leaving
    /// the handle set would render whatever brick the pool slot is reassigned
    /// to at this location.
    pub fn remove_brick(&mut self, world_pos: Vec3, leaf_size: f32) {
        let (cell, depth) = self.quantize_cell(world_pos, leaf_size);

        let mut node_idx = self.root;
        for level in (0..depth).rev() {
            let node = self.nodes[node_idx as usize];
            let octant = Self::octant_at(cell, level);
            if node.children == 0 || node.flags & (1 << octant) == 0 {
                return;
            }
            node_idx = node.children + octant;
        }

        let node = &mut self.nodes[node_idx as usize];
        if node.flags & (1 << 8) != 0 {
            node.brick = u32::MAX;
            node.flags &= !(1 << 8);
            self.dirty.push(node_idx);
        }
    }

    /// Clears the leaf at `world_pos` entirely — color, brick, everything.
    /// Called when streaming discovers a coarsely-solid cell is actually air
    /// (e.g. a cave breach the heightfield estimate could not see): the
    /// coarse color must go, or an opaque box floats over the real terrain.
    /// Descends with allocation so a cell inside a coarse solid node
    /// subdivides it (materializing its siblings) before clearing.
    pub fn clear_leaf(&mut self, world_pos: Vec3, leaf_size: f32) {
        let (cell, depth) = self.quantize_cell(world_pos, leaf_size);

        let mut node_idx = self.root;
        for level in (0..depth).rev() {
            let octant = Self::octant_at(cell, level) as u8;
            node_idx = self.alloc_child(node_idx, octant);
        }

        let node = &mut self.nodes[node_idx as usize];
        node.color = 0;
        node.brick = u32::MAX;
        node.flags &= !(1 << 8);
        self.dirty.push(node_idx);

        self.refresh_path_colors(cell, depth);
    }

    /// Returns `(packed_color, has_brick)` of the leaf at `world_pos`, or
    /// `None` if no node exists along the path. Read-only query for tests
    /// and tooling.
    #[must_use]
    pub fn leaf_info(&self, world_pos: Vec3, leaf_size: f32) -> Option<(u32, bool)> {
        let (cell, depth) = self.quantize_cell(world_pos, leaf_size);

        let mut node_idx = self.root;
        for level in (0..depth).rev() {
            let node = self.nodes[node_idx as usize];
            let octant = Self::octant_at(cell, level);
            if node.children == 0 || node.flags & (1 << octant) == 0 {
                // A childless colored ancestor covers this cell (coarse solid).
                if node.children == 0 && (node.color >> 24) & 0xFF > 0 {
                    return Some((node.color, false));
                }
                return None;
            }
            node_idx = node.children + octant;
        }
        let node = self.nodes[node_idx as usize];
        Some((node.color, node.flags & (1 << 8) != 0))
    }

    /// Raw node fields along the root→leaf path of a cell, for debugging.
    /// Stops early where the path leaves the built tree.
    #[doc(hidden)]
    #[must_use]
    pub fn debug_path(&self, world_pos: Vec3, leaf_size: f32) -> Vec<(u32, u32, u32, u32)> {
        let (cell, depth) = self.quantize_cell(world_pos, leaf_size);
        let mut out = Vec::new();
        let mut node_idx = self.root;
        for level in (0..depth).rev() {
            let node = self.nodes[node_idx as usize];
            out.push((node_idx, node.children, node.color, node.flags));
            let octant = Self::octant_at(cell, level);
            if node.children == 0 || node.flags & (1 << octant) == 0 {
                return out;
            }
            node_idx = node.children + octant;
        }
        let node = self.nodes[node_idx as usize];
        out.push((node_idx, node.children, node.color, node.flags));
        out
    }

    /// Recomputes interior colors bottom-up along the root→leaf path of one
    /// cell. O(depth × 8) — the streaming-time replacement for the full-tree
    /// [`update_colors`](Self::update_colors) pass.
    fn refresh_path_colors(&mut self, cell: [u32; 3], depth: u32) {
        let mut path = [0u32; 32];
        let mut len = 0usize;
        let mut node_idx = self.root;

        for level in (0..depth).rev() {
            path[len] = node_idx;
            len += 1;
            let node = self.nodes[node_idx as usize];
            let octant = Self::octant_at(cell, level);
            if node.children == 0 || node.flags & (1 << octant) == 0 {
                len -= 1;
                break;
            }
            node_idx = node.children + octant;
        }

        for i in (0..len).rev() {
            self.recompute_color(path[i]);
        }
    }

    /// Recomputes one interior node's color as the unweighted average of its
    /// direct children's colors (children with zero alpha are skipped).
    fn recompute_color(&mut self, idx: u32) {
        let node = self.nodes[idx as usize];
        if !node.has_children() {
            return;
        }

        let mut r = 0u32;
        let mut g = 0u32;
        let mut b = 0u32;
        let mut a = 0u32;
        let mut n = 0u32;
        for i in 0..8u32 {
            if node.child_mask() & (1 << i) != 0 {
                let c = self.nodes[(node.children + i) as usize].color;
                let ca = (c >> 24) & 0xFF;
                if ca > 0 {
                    r += c & 0xFF;
                    g += (c >> 8) & 0xFF;
                    b += (c >> 16) & 0xFF;
                    a += ca;
                    n += 1;
                }
            }
        }

        // n == 0 (all children invisible) must zero the color: a stale
        // opaque color would keep rendering at SSE-coarse distances.
        let avg = |sum: u32| sum.checked_div(n).unwrap_or(0) as u8;
        let color = pack_rgba([avg(r), avg(g), avg(b), avg(a)]);
        if self.nodes[idx as usize].color != color {
            self.nodes[idx as usize].color = color;
            self.dirty.push(idx);
        }
    }

    /// Recomputes all interior node colors bottom-up from leaves.
    /// Full-tree pass — use once after bulk construction; streaming updates
    /// go through the path-local refresh inside `insert_brick`.
    pub fn update_colors(&mut self) {
        self.update_colors_recursive(self.root);
    }

    fn update_colors_recursive(&mut self, idx: u32) {
        let node = self.nodes[idx as usize];
        if !node.has_children() {
            return;
        }
        for i in 0..8u32 {
            if node.child_mask() & (1 << i) != 0 {
                self.update_colors_recursive(node.children + i);
            }
        }
        self.recompute_color(idx);
    }

    /// Uploads the entire CPU node array to the GPU buffer and clears the
    /// dirty set. Use after bulk construction; per-frame streaming uses
    /// [`upload_dirty`](Self::upload_dirty).
    pub fn upload(&mut self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.nodes));
        self.dirty.clear();
    }

    /// Uploads only nodes modified since the last upload, coalescing nearby
    /// indices into ranged writes. Per-frame cost is proportional to the
    /// number of touched nodes, not tree size.
    pub fn upload_dirty(&mut self, queue: &wgpu::Queue) {
        if self.dirty.is_empty() {
            return;
        }
        self.dirty.sort_unstable();
        self.dirty.dedup();

        // Merge ranges separated by small gaps: one larger write beats many
        // tiny ones (each write_buffer is a staging alloc + copy).
        const MERGE_GAP: u32 = 64;
        let node_size = size_of::<SvoNode>() as u64;
        let dirty = std::mem::take(&mut self.dirty);

        let mut start = dirty[0];
        let mut end = start + 1;
        for &idx in &dirty[1..] {
            if idx < end + MERGE_GAP {
                end = idx + 1;
            } else {
                queue.write_buffer(
                    &self.buffer,
                    u64::from(start) * node_size,
                    bytemuck::cast_slice(&self.nodes[start as usize..end as usize]),
                );
                start = idx;
                end = idx + 1;
            }
        }
        queue.write_buffer(
            &self.buffer,
            u64::from(start) * node_size,
            bytemuck::cast_slice(&self.nodes[start as usize..end as usize]),
        );
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
        let block = self.free_blocks.pop().unwrap_or_else(|| {
            panic!(
                "SVO node pool exhausted at capacity {} — increase the preset's svo_capacity",
                self.capacity
            )
        });
        for i in 0..CHILD_BLOCK {
            self.nodes[(block + i) as usize] = SvoNode::EMPTY;
            self.dirty.push(block + i);
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

    /// Regression test for sw-fcea39: eviction must detach the brick handle
    /// (or the reassigned pool slot's new contents render at the old
    /// location) while keeping the averaged color as LOD fallback.
    #[test]
    fn remove_brick_keeps_color_clears_handle() {
        let mut svo = test_svo();
        let pos = Vec3::new(1.0, 1.0, 1.0);
        svo.insert_brick(pos, 1.0, BrickHandle::NONE, [10, 20, 30, 255]);
        svo.remove_brick(pos, 1.0);

        let (cell, depth) = svo.quantize_cell(pos, 1.0);
        let mut idx = svo.root;
        for level in (0..depth).rev() {
            let node = svo.nodes[idx as usize];
            let octant = Svo::octant_at(cell, level);
            assert!(node.children != 0 && node.flags & (1 << octant) != 0);
            idx = node.children + octant;
        }
        let leaf = svo.nodes[idx as usize];
        assert_eq!(leaf.flags & (1 << 8), 0, "brick flag must be cleared");
        assert_eq!(leaf.brick, u32::MAX, "handle must be detached");
        assert_eq!(leaf.color & 0xFF, 10, "color must survive as LOD fallback");
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
