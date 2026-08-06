//! Flat-array BVH over instance AABBs for GPU ray traversal.

use glam::Vec3;

/// A BVH node in flat array layout.
///
/// Internal nodes: `count == 0`, children at indices `left_or_first` and
/// `left_or_first + 1`.
/// Leaf nodes: `count > 0`, instances at indices `left_or_first ..
/// left_or_first + count`.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BvhNode {
    /// Minimum corner of this node's bounding box.
    pub aabb_min: [f32; 3],
    /// Internal: left child index. Leaf: first instance index.
    pub left_or_first: u32,
    /// Maximum corner of this node's bounding box.
    pub aabb_max: [f32; 3],
    /// 0 = internal node, >0 = leaf with this many instances.
    pub count: u32,
}

/// Builds a BVH from a list of AABBs. Returns the node array and a
/// reordered index array mapping leaf entries back to original instances.
pub fn build(aabbs: &[(Vec3, Vec3)]) -> (Vec<BvhNode>, Vec<u32>) {
    let n = aabbs.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut indices: Vec<u32> = (0..n as u32).collect();
    let mut nodes: Vec<BvhNode> = Vec::with_capacity(2 * n);

    // Centroids for sorting
    let centroids: Vec<Vec3> = aabbs.iter().map(|(lo, hi)| (*lo + *hi) * 0.5).collect();

    // Root node
    let (root_min, root_max) = aabb_of_range(aabbs, &indices, 0, n);
    nodes.push(BvhNode {
        aabb_min: root_min.into(),
        left_or_first: 0,
        aabb_max: root_max.into(),
        count: n as u32,
    });

    subdivide(&mut nodes, &mut indices, aabbs, &centroids, 0);

    (nodes, indices)
}

const MAX_LEAF_SIZE: usize = 4;

fn subdivide(
    nodes: &mut Vec<BvhNode>,
    indices: &mut [u32],
    aabbs: &[(Vec3, Vec3)],
    centroids: &[Vec3],
    node_idx: usize,
) {
    let first = nodes[node_idx].left_or_first as usize;
    let count = nodes[node_idx].count as usize;

    if count <= MAX_LEAF_SIZE {
        return;
    }

    // Find split axis: longest extent of centroid bounds
    let mut c_min = Vec3::splat(f32::MAX);
    let mut c_max = Vec3::splat(f32::MIN);
    for i in first..first + count {
        let c = centroids[indices[i] as usize];
        c_min = c_min.min(c);
        c_max = c_max.max(c);
    }
    let extent = c_max - c_min;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };

    let mid = (c_min[axis] + c_max[axis]) * 0.5;

    // Partition indices around midpoint
    let mut i = first;
    let mut j = first + count - 1;
    while i <= j {
        if centroids[indices[i] as usize][axis] < mid {
            i += 1;
        } else {
            indices.swap(i, j);
            if j == 0 {
                break;
            }
            j -= 1;
        }
    }

    let left_count = i - first;
    if left_count == 0 || left_count == count {
        // Degenerate split — force half/half
        let half = count / 2;
        let left_count = half;
        let right_count = count - half;
        let left_idx = nodes.len();

        let (l_min, l_max) = aabb_of_range(aabbs, indices, first, left_count);
        let (r_min, r_max) = aabb_of_range(aabbs, indices, first + left_count, right_count);

        nodes[node_idx].left_or_first = left_idx as u32;
        nodes[node_idx].count = 0;

        nodes.push(BvhNode {
            aabb_min: l_min.into(),
            left_or_first: first as u32,
            aabb_max: l_max.into(),
            count: left_count as u32,
        });
        nodes.push(BvhNode {
            aabb_min: r_min.into(),
            left_or_first: (first + left_count) as u32,
            aabb_max: r_max.into(),
            count: right_count as u32,
        });

        subdivide(nodes, indices, aabbs, centroids, left_idx);
        subdivide(nodes, indices, aabbs, centroids, left_idx + 1);
        return;
    }

    let right_count = count - left_count;
    let left_idx = nodes.len();

    let (l_min, l_max) = aabb_of_range(aabbs, indices, first, left_count);
    let (r_min, r_max) = aabb_of_range(aabbs, indices, first + left_count, right_count);

    nodes[node_idx].left_or_first = left_idx as u32;
    nodes[node_idx].count = 0;

    nodes.push(BvhNode {
        aabb_min: l_min.into(),
        left_or_first: first as u32,
        aabb_max: l_max.into(),
        count: left_count as u32,
    });
    nodes.push(BvhNode {
        aabb_min: r_min.into(),
        left_or_first: (first + left_count) as u32,
        aabb_max: r_max.into(),
        count: right_count as u32,
    });

    subdivide(nodes, indices, aabbs, centroids, left_idx);
    subdivide(nodes, indices, aabbs, centroids, left_idx + 1);
}

fn aabb_of_range(
    aabbs: &[(Vec3, Vec3)],
    indices: &[u32],
    start: usize,
    count: usize,
) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for i in start..start + count {
        let (a, b) = aabbs[indices[i] as usize];
        lo = lo.min(a);
        hi = hi.max(b);
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_build() {
        let (nodes, indices) = build(&[]);
        assert!(nodes.is_empty());
        assert!(indices.is_empty());
    }

    #[test]
    fn single_instance() {
        let aabbs = vec![(Vec3::ZERO, Vec3::ONE)];
        let (nodes, indices) = build(&aabbs);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].count, 1);
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn many_instances_produces_tree() {
        let aabbs: Vec<(Vec3, Vec3)> = (0..20)
            .map(|i| {
                let x = i as f32;
                (Vec3::new(x, 0.0, 0.0), Vec3::new(x + 1.0, 1.0, 1.0))
            })
            .collect();
        let (nodes, indices) = build(&aabbs);
        assert!(nodes.len() > 1);
        assert_eq!(indices.len(), 20);
        // All original indices present
        let mut sorted = indices.clone();
        sorted.sort();
        assert_eq!(sorted, (0..20).collect::<Vec<u32>>());
    }

    #[test]
    fn root_aabb_encloses_all() {
        let aabbs = vec![
            (Vec3::new(-5.0, -5.0, -5.0), Vec3::new(-4.0, -4.0, -4.0)),
            (Vec3::new(4.0, 4.0, 4.0), Vec3::new(5.0, 5.0, 5.0)),
        ];
        let (nodes, _) = build(&aabbs);
        let root = &nodes[0];
        assert!(root.aabb_min[0] <= -5.0);
        assert!(root.aabb_max[0] >= 5.0);
    }
}
