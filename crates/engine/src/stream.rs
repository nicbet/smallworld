//! Stream stage: second stage of the OOC pipeline.
//!
//! Ensures visible geometry has GPU buffers ready for Execute. Volumes
//! get async mesh extraction on the job pool; mesh assets get uploaded
//! from CPU data. Never blocks the frame — renders whatever's cached.
//!
//! Includes upload budget (cap per frame), priority ordering (closest
//! volumes first), and priority-based eviction with memory budget.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::Vec3;

use crate::cull::VisibilitySet;
use crate::jobs::{Scheduler, TaskHandle};
use crate::mesh::Vertex;
use crate::volume::AABB;
use crate::world::{LightKey, MeshInstanceKey, MeshKey, VolumeKey, World};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// CPU-side extraction result: vertices + indices ready for GPU upload.
pub struct MeshData {
    /// Vertex data.
    pub vertices: Vec<Vertex>,
    /// Triangle index data.
    pub indices: Vec<u32>,
}

/// GPU-ready vertex + index buffers.
pub struct GpuMesh {
    /// Vertex buffer.
    pub vertex_buffer: wgpu::Buffer,
    /// Index buffer.
    pub index_buffer: wgpu::Buffer,
    /// Number of indices (for draw calls).
    pub index_count: u32,
    /// Total GPU bytes (vertex + index buffers) for budget tracking.
    pub byte_size: u64,
}

impl GpuMesh {
    /// Uploads `MeshData` to the GPU. Returns `None` if the mesh is empty.
    pub fn upload(device: &wgpu::Device, data: &MeshData) -> Option<Self> {
        if data.vertices.is_empty() || data.indices.is_empty() {
            return None;
        }

        let vb_size = (data.vertices.len() * size_of::<Vertex>()) as u64;
        let ib_size = (data.indices.len() * size_of::<u32>()) as u64;

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stream_vertex"),
            size: vb_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let mut view = vertex_buffer
                .slice(..)
                .get_mapped_range_mut()
                .expect("failed to map vertex buffer");
            view.copy_from_slice(bytemuck::cast_slice(&data.vertices));
        }
        vertex_buffer.unmap();

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stream_index"),
            size: ib_size,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let mut view = index_buffer
                .slice(..)
                .get_mapped_range_mut()
                .expect("failed to map index buffer");
            view.copy_from_slice(bytemuck::cast_slice(&data.indices));
        }
        index_buffer.unmap();

        Some(Self {
            vertex_buffer,
            index_buffer,
            index_count: data.indices.len() as u32,
            byte_size: vb_size + ib_size,
        })
    }
}

// ---------------------------------------------------------------------------
// MeshExtractor trait + placeholder
// ---------------------------------------------------------------------------

/// Extracts triangle meshes from volume data on worker threads.
///
/// Receives key + AABB (not `&dyn VoxelVolume`) because extraction runs
/// on background threads and volume trait objects may hold GPU resources.
pub trait MeshExtractor: Send + Sync {
    /// Extracts a triangle mesh for the given volume.
    fn extract(&self, key: VolumeKey, bounds: AABB) -> MeshData;
}

/// Generates a box mesh from a volume's AABB. Temporary — replaced by
/// real extractors that access CPU-side voxel data.
pub struct PlaceholderExtractor;

impl MeshExtractor for PlaceholderExtractor {
    fn extract(&self, _key: VolumeKey, bounds: AABB) -> MeshData {
        box_mesh(bounds)
    }
}

fn box_mesh(aabb: AABB) -> MeshData {
    let min = aabb.min;
    let max = aabb.max;

    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        // +Y (top)
        (
            [0.0, 1.0, 0.0],
            [
                [min[0], max[1], min[2]],
                [max[0], max[1], min[2]],
                [max[0], max[1], max[2]],
                [min[0], max[1], max[2]],
            ],
        ),
        // -Y (bottom)
        (
            [0.0, -1.0, 0.0],
            [
                [min[0], min[1], max[2]],
                [max[0], min[1], max[2]],
                [max[0], min[1], min[2]],
                [min[0], min[1], min[2]],
            ],
        ),
        // +X (right)
        (
            [1.0, 0.0, 0.0],
            [
                [max[0], min[1], min[2]],
                [max[0], min[1], max[2]],
                [max[0], max[1], max[2]],
                [max[0], max[1], min[2]],
            ],
        ),
        // -X (left)
        (
            [-1.0, 0.0, 0.0],
            [
                [min[0], min[1], max[2]],
                [min[0], min[1], min[2]],
                [min[0], max[1], min[2]],
                [min[0], max[1], max[2]],
            ],
        ),
        // +Z (front)
        (
            [0.0, 0.0, 1.0],
            [
                [min[0], min[1], max[2]],
                [max[0], min[1], max[2]],
                [max[0], max[1], max[2]],
                [min[0], max[1], max[2]],
            ],
        ),
        // -Z (back)
        (
            [0.0, 0.0, -1.0],
            [
                [max[0], min[1], min[2]],
                [min[0], min[1], min[2]],
                [min[0], max[1], min[2]],
                [max[0], max[1], min[2]],
            ],
        ),
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    for (normal, corners) in &faces {
        let base = vertices.len() as u32;
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for (i, pos) in corners.iter().enumerate() {
            vertices.push(Vertex {
                position: *pos,
                normal: *normal,
                uv: uvs[i],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    MeshData { vertices, indices }
}

// ---------------------------------------------------------------------------
// MeshCache
// ---------------------------------------------------------------------------

struct VolumeCacheEntry {
    gpu_mesh: GpuMesh,
    last_camera_distance: f32,
}

/// Caches GPU-ready meshes for volumes and mesh assets. Evicts farthest
/// entries when the total byte budget is exceeded.
pub struct MeshCache {
    volume_meshes: HashMap<VolumeKey, VolumeCacheEntry>,
    mesh_assets: HashMap<MeshKey, GpuMesh>,
    total_bytes: u64,
    budget_bytes: u64,
}

impl MeshCache {
    fn new(budget_bytes: u64) -> Self {
        Self {
            volume_meshes: HashMap::new(),
            mesh_assets: HashMap::new(),
            total_bytes: 0,
            budget_bytes,
        }
    }

    /// Returns the GPU mesh for a volume, if cached.
    pub fn get_volume_mesh(&self, key: VolumeKey) -> Option<&GpuMesh> {
        self.volume_meshes.get(&key).map(|e| &e.gpu_mesh)
    }

    /// Returns the GPU mesh for a mesh asset, if cached.
    pub fn get_mesh_asset(&self, key: MeshKey) -> Option<&GpuMesh> {
        self.mesh_assets.get(&key)
    }

    fn insert_volume_mesh(&mut self, key: VolumeKey, gpu_mesh: GpuMesh, camera_distance: f32) {
        self.total_bytes += gpu_mesh.byte_size;
        self.volume_meshes.insert(
            key,
            VolumeCacheEntry {
                gpu_mesh,
                last_camera_distance: camera_distance,
            },
        );
    }

    fn insert_mesh_asset(&mut self, key: MeshKey, gpu_mesh: GpuMesh) {
        self.total_bytes += gpu_mesh.byte_size;
        self.mesh_assets.insert(key, gpu_mesh);
    }

    fn update_distance(&mut self, key: VolumeKey, distance: f32) {
        if let Some(entry) = self.volume_meshes.get_mut(&key) {
            entry.last_camera_distance = distance;
        }
    }

    fn evict_over_budget(&mut self) {
        while self.total_bytes > self.budget_bytes && !self.volume_meshes.is_empty() {
            let farthest = self
                .volume_meshes
                .iter()
                .max_by(|a, b| {
                    a.1.last_camera_distance
                        .partial_cmp(&b.1.last_camera_distance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(&k, _)| k);

            if let Some(key) = farthest {
                if let Some(entry) = self.volume_meshes.remove(&key) {
                    self.total_bytes = self.total_bytes.saturating_sub(entry.gpu_mesh.byte_size);
                }
            } else {
                break;
            }
        }
    }

    /// Total GPU bytes tracked by this cache.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Number of cached volume meshes.
    #[must_use]
    pub fn volume_mesh_count(&self) -> usize {
        self.volume_meshes.len()
    }

    /// Number of cached mesh assets.
    #[must_use]
    pub fn mesh_asset_count(&self) -> usize {
        self.mesh_assets.len()
    }
}

// ---------------------------------------------------------------------------
// StreamStage
// ---------------------------------------------------------------------------

struct ReadyEntry {
    key: VolumeKey,
    mesh_data: MeshData,
    camera_distance: f32,
}

/// Default memory budget: 256 MB.
const DEFAULT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

/// Default per-frame upload cap: 8 MB.
const DEFAULT_UPLOAD_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

type ExtractionResult = (VolumeKey, MeshData, f32);

/// Second pipeline stage. Manages async mesh extraction, GPU upload,
/// caching, and eviction.
pub struct StreamStage {
    cache: MeshCache,
    extractor: Arc<dyn MeshExtractor>,
    pending: HashSet<VolumeKey>,
    pending_handles: Vec<(VolumeKey, TaskHandle<ExtractionResult>)>,
    ready_queue: Vec<ReadyEntry>,
    upload_budget_bytes: u64,
}

/// Output of the stream stage: references to cached GPU meshes for
/// everything that's ready to render this frame.
pub struct StreamOutput<'a> {
    /// Visible volumes with cached GPU meshes.
    pub volume_meshes: Vec<(VolumeKey, &'a GpuMesh)>,
    /// Visible mesh instances with cached GPU meshes.
    pub mesh_instances: Vec<(MeshInstanceKey, &'a GpuMesh)>,
    /// Visible lights (passed through unchanged).
    pub lights: Vec<LightKey>,
}

impl StreamStage {
    /// Creates a stream stage with the given extractor.
    pub fn new(extractor: Arc<dyn MeshExtractor>) -> Self {
        Self {
            cache: MeshCache::new(DEFAULT_BUDGET_BYTES),
            extractor,
            pending: HashSet::new(),
            pending_handles: Vec::new(),
            ready_queue: Vec::new(),
            upload_budget_bytes: DEFAULT_UPLOAD_BUDGET_BYTES,
        }
    }

    /// Runs the stream stage for this frame. Called by Engine, not by games.
    pub(crate) fn stream<'a>(
        &'a mut self,
        world: &World,
        visibility: &VisibilitySet,
        scheduler: &impl Scheduler,
        device: &wgpu::Device,
        camera_pos: Vec3,
    ) -> StreamOutput<'a> {
        // 1. Drain completed extraction handles into ready_queue
        let mut still_pending = Vec::new();
        for (key, handle) in self.pending_handles.drain(..) {
            if handle.is_complete() {
                let (_, mesh_data, distance) = handle.wait();
                self.pending.remove(&key);
                self.ready_queue.push(ReadyEntry {
                    key,
                    mesh_data,
                    camera_distance: distance,
                });
            } else {
                still_pending.push((key, handle));
            }
        }
        self.pending_handles = still_pending;

        // 2. Upload from ready_queue up to budget
        let mut bytes_uploaded: u64 = 0;
        let mut remaining = Vec::new();

        for entry in self.ready_queue.drain(..) {
            let estimated_size = (entry.mesh_data.vertices.len() * size_of::<Vertex>()
                + entry.mesh_data.indices.len() * size_of::<u32>())
                as u64;

            if bytes_uploaded > 0 && bytes_uploaded + estimated_size > self.upload_budget_bytes {
                remaining.push(entry);
                continue;
            }

            if let Some(gpu_mesh) = GpuMesh::upload(device, &entry.mesh_data) {
                bytes_uploaded += gpu_mesh.byte_size;
                self.cache
                    .insert_volume_mesh(entry.key, gpu_mesh, entry.camera_distance);
            }
        }
        self.ready_queue = remaining;

        // 3. Submit extraction jobs for uncached visible volumes (priority ordered)
        let mut uncached: Vec<(VolumeKey, AABB, f32)> = Vec::new();
        for &key in &visibility.volumes {
            if self.cache.get_volume_mesh(key).is_some() {
                let distance = volume_distance(world, key, camera_pos);
                self.cache.update_distance(key, distance);
            } else if !self.pending.contains(&key)
                && !self.ready_queue.iter().any(|e| e.key == key)
                && let Some(vol) = world.volume(key)
            {
                let bounds = vol.bounds();
                let distance = aabb_distance(&bounds, camera_pos);
                uncached.push((key, bounds, distance));
            }
        }

        uncached.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

        for (key, bounds, distance) in uncached {
            self.pending.insert(key);
            let extractor = Arc::clone(&self.extractor);
            let handle = scheduler.spawn(move || {
                let mesh_data = extractor.extract(key, bounds);
                (key, mesh_data, distance)
            });
            self.pending_handles.push((key, handle));
        }

        // 4. Upload mesh assets for visible instances (within remaining budget)
        for &key in &visibility.mesh_instances {
            if let Some(inst) = world.mesh_instance(key) {
                let mesh_key = inst.mesh;
                if self.cache.get_mesh_asset(mesh_key).is_none()
                    && let Some(mesh) = world.mesh(mesh_key)
                {
                    let mesh_data = MeshData {
                        vertices: mesh.vertices.clone(),
                        indices: mesh.indices.clone(),
                    };
                    let estimated = (mesh_data.vertices.len() * size_of::<Vertex>()
                        + mesh_data.indices.len() * size_of::<u32>())
                        as u64;

                    if bytes_uploaded > 0
                        && bytes_uploaded + estimated > self.upload_budget_bytes
                    {
                        continue;
                    }

                    if let Some(gpu_mesh) = GpuMesh::upload(device, &mesh_data) {
                        bytes_uploaded += gpu_mesh.byte_size;
                        self.cache.insert_mesh_asset(mesh_key, gpu_mesh);
                    }
                }
            }
        }

        // 5. Evict if over memory budget
        self.cache.evict_over_budget();

        // 6. Build output
        self.build_output(world, visibility)
    }

    fn build_output<'a>(&'a self, world: &World, visibility: &VisibilitySet) -> StreamOutput<'a> {
        let volume_meshes = visibility
            .volumes
            .iter()
            .filter_map(|&key| self.cache.get_volume_mesh(key).map(|mesh| (key, mesh)))
            .collect();

        let mesh_instances = visibility
            .mesh_instances
            .iter()
            .filter_map(|&key| {
                let inst = world.mesh_instance(key)?;
                let gpu_mesh = self.cache.get_mesh_asset(inst.mesh)?;
                Some((key, gpu_mesh))
            })
            .collect();

        StreamOutput {
            volume_meshes,
            mesh_instances,
            lights: visibility.lights.clone(),
        }
    }

    /// Access to the mesh cache (for debug/inspection).
    #[must_use]
    pub fn cache(&self) -> &MeshCache {
        &self.cache
    }
}

fn volume_distance(world: &World, key: VolumeKey, camera_pos: Vec3) -> f32 {
    world
        .volume(key)
        .map(|v| aabb_distance(&v.bounds(), camera_pos))
        .unwrap_or(f32::MAX)
}

fn aabb_distance(aabb: &AABB, point: Vec3) -> f32 {
    let min = aabb.min_vec3();
    let max = aabb.max_vec3();
    let clamped = point.clamp(min, max);
    point.distance(clamped)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placeholder_extractor_produces_box() {
        let extractor = PlaceholderExtractor;
        let aabb = AABB::new(Vec3::ZERO, Vec3::ONE);
        let mut sm = slotmap::SlotMap::<VolumeKey, ()>::with_key();
        let key = sm.insert(());
        let data = extractor.extract(key, aabb);
        assert_eq!(data.vertices.len(), 24);
        assert_eq!(data.indices.len(), 36);
    }

    #[test]
    fn box_mesh_normals_are_unit() {
        let data = box_mesh(AABB::new(Vec3::ZERO, Vec3::ONE));
        for v in &data.vertices {
            let n = Vec3::from(v.normal);
            assert!(
                (n.length() - 1.0).abs() < 1e-5,
                "non-unit normal: {n:?}"
            );
        }
    }

    #[test]
    fn box_mesh_respects_bounds() {
        let min = Vec3::new(-2.0, 0.0, -3.0);
        let max = Vec3::new(4.0, 5.0, 1.0);
        let data = box_mesh(AABB::new(min, max));
        for v in &data.vertices {
            let p = Vec3::from(v.position);
            assert!(p.x >= min.x - 1e-5 && p.x <= max.x + 1e-5);
            assert!(p.y >= min.y - 1e-5 && p.y <= max.y + 1e-5);
            assert!(p.z >= min.z - 1e-5 && p.z <= max.z + 1e-5);
        }
    }

    #[test]
    fn aabb_distance_inside_is_zero() {
        let aabb = AABB::new(Vec3::ZERO, Vec3::splat(10.0));
        let d = aabb_distance(&aabb, Vec3::splat(5.0));
        assert!((d - 0.0).abs() < 1e-5);
    }

    #[test]
    fn aabb_distance_outside() {
        let aabb = AABB::new(Vec3::ZERO, Vec3::ONE);
        let d = aabb_distance(&aabb, Vec3::new(2.0, 0.5, 0.5));
        assert!((d - 1.0).abs() < 1e-5);
    }

    #[test]
    fn priority_sort_closest_first() {
        let mut items = vec![("far", 100.0_f32), ("close", 1.0), ("mid", 50.0)];
        items.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        assert_eq!(items[0].0, "close");
        assert_eq!(items[1].0, "mid");
        assert_eq!(items[2].0, "far");
    }

    #[test]
    fn cache_empty_by_default() {
        let cache = MeshCache::new(1024 * 1024);
        assert_eq!(cache.total_bytes(), 0);
        assert_eq!(cache.volume_mesh_count(), 0);
        assert_eq!(cache.mesh_asset_count(), 0);
    }

    #[test]
    fn cache_miss_returns_none() {
        let cache = MeshCache::new(1024 * 1024);
        let mut sm = slotmap::SlotMap::<VolumeKey, ()>::with_key();
        let key = sm.insert(());
        assert!(cache.get_volume_mesh(key).is_none());
    }

    #[test]
    fn stream_stage_creates_with_extractor() {
        let stage = StreamStage::new(Arc::new(PlaceholderExtractor));
        assert_eq!(stage.cache().total_bytes(), 0);
        assert_eq!(stage.cache().volume_mesh_count(), 0);
    }
}
