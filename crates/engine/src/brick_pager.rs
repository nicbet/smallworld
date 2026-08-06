//! Async brick pager: streams brick data from a [`BrickSource`] into the GPU
//! on demand, with residency tracking and LRU eviction under a hard VRAM budget.

use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use glam::Vec3;

use crate::brick_data::BrickData;
use crate::brick_index::{BRICK_SIZE, BrickIndex};
use crate::brick_pool::BrickPool;
use crate::brick_source::BrickSource;
use crate::coarse_mip_grid::CoarseMipGrid;
use crate::mip;

const REQUEST_CHANNEL_CAP: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellState {
    /// Never loaded or evicted — eligible for load requests.
    Unknown,
    /// Confirmed empty — never re-request.
    Air,
    /// Load in progress on a background thread.
    Loading { existing_slot: Option<u32> },
    /// Full voxel + palette + mip data in VRAM.
    Resident { slot: u32 },
    /// Only mip data valid; voxels/palette stale. Eviction candidate.
    MipOnly { slot: u32 },
}

/// Pager configuration.
pub struct PagerConfig {
    /// Maximum brick uploads per frame (default 512).
    pub max_uploads_per_frame: u32,
    /// Background worker thread count (default 4).
    pub worker_threads: usize,
}

impl Default for PagerConfig {
    fn default() -> Self {
        Self {
            max_uploads_per_frame: 128,
            worker_threads: 4,
        }
    }
}

/// Per-frame statistics from the pager.
#[derive(Clone, Copy, Default, Debug)]
pub struct PagerStats {
    /// Bricks with full data in VRAM.
    pub resident: u32,
    /// Bricks with only mip data valid.
    pub mip_only: u32,
    /// Bricks being loaded on background threads.
    pub loading: u32,
    /// Grid cells not yet loaded.
    pub unknown: u32,
    /// Grid cells confirmed empty (air).
    pub air: u32,
    /// Bricks evicted this frame.
    pub evicted_this_frame: u32,
    /// Bricks uploaded to GPU this frame.
    pub uploaded_this_frame: u32,
}

struct LoadResult {
    grid_pos: [u32; 3],
    data: Option<BrickData>,
    mips: [u32; mip::MIP_WORDS_PER_BRICK as usize],
}

/// Async brick pager: manages residency, background loading, and LRU eviction.
///
/// Call [`update()`](Self::update) once per frame. It drains completed loads,
/// uploads bricks to the GPU, computes demand from the camera, and dispatches
/// new load requests to worker threads.
///
/// For initial scene setup, call [`preload_all()`](Self::preload_all) to block
/// until the entire grid is populated before the first frame.
pub struct BrickPager {
    cell_states: Vec<CellState>,
    slot_last_used: Vec<u64>,
    slot_cell: Vec<Option<usize>>,
    in_flight: HashSet<[u32; 3]>,
    request_tx: Option<Sender<[u32; 3]>>,
    result_rx: Receiver<LoadResult>,
    workers: Vec<Option<thread::JoinHandle<()>>>,
    dims: [u32; 3],
    world_min: Vec3,
    frame: u64,
    config: PagerConfig,
}

impl BrickPager {
    /// Creates a new pager and spawns background worker threads.
    pub fn new(
        source: Arc<dyn BrickSource>,
        dims: [u32; 3],
        world_min: Vec3,
        pool_capacity: u32,
        config: PagerConfig,
    ) -> Self {
        let total_cells = (dims[0] * dims[1] * dims[2]) as usize;
        let cell_states = vec![CellState::Unknown; total_cells];
        let slot_last_used = vec![0u64; pool_capacity as usize];
        let slot_cell = vec![None; pool_capacity as usize];

        let (request_tx, request_rx) = crossbeam_channel::bounded(REQUEST_CHANNEL_CAP);
        let (result_tx, result_rx) = crossbeam_channel::unbounded();

        let workers = Self::spawn_workers(
            source,
            world_min,
            request_rx,
            result_tx,
            config.worker_threads,
        );

        log::info!(
            "brick pager: {} cells, {} workers, max {} uploads/frame",
            total_cells,
            config.worker_threads,
            config.max_uploads_per_frame,
        );

        Self {
            cell_states,
            slot_last_used,
            slot_cell,
            in_flight: HashSet::new(),
            request_tx: Some(request_tx),
            result_rx,
            workers,
            dims,
            world_min,
            frame: 0,
            config,
        }
    }

    /// Blocks until every grid cell has been loaded or confirmed air.
    ///
    /// Call once during scene setup so terrain is fully populated before the
    /// first frame. Workers run in parallel; this method just waits for them
    /// and uploads results as they arrive.
    pub fn preload_all(
        &mut self,
        index: &mut BrickIndex,
        pool: &mut BrickPool,
        coarse: &mut CoarseMipGrid,
        queue: &wgpu::Queue,
    ) {
        let start = Instant::now();
        let mut total_to_load = 0u32;

        if let Some(tx) = &self.request_tx {
            for gz in 0..self.dims[2] {
                for gy in 0..self.dims[1] {
                    for gx in 0..self.dims[0] {
                        let grid_pos = [gx, gy, gz];
                        let flat = self.flat_index(grid_pos);
                        if self.cell_states[flat] == CellState::Unknown {
                            tx.send(grid_pos).expect("worker channel closed");
                            self.cell_states[flat] = CellState::Loading {
                                existing_slot: None,
                            };
                            total_to_load += 1;
                        }
                    }
                }
            }
        }

        let mut loaded = 0u32;
        let mut air = 0u32;
        for _ in 0..total_to_load {
            let result = self.result_rx.recv().expect("worker channel closed");
            let flat = self.flat_index(result.grid_pos);

            match result.data {
                None => {
                    self.cell_states[flat] = CellState::Air;
                    air += 1;
                }
                Some(data) => {
                    let handle = pool.alloc().expect("pool exhausted during preload");
                    pool.write_voxels(queue, handle, &data.voxels);
                    pool.write_palette(queue, handle, &data.palette);
                    pool.write_mips(queue, handle, &result.mips);
                    coarse.write_cell(result.grid_pos, &result.mips);
                    index.set(result.grid_pos, handle);

                    let slot = handle.gpu_index();
                    self.cell_states[flat] = CellState::Resident { slot };
                    self.slot_last_used[slot as usize] = self.frame;
                    self.slot_cell[slot as usize] = Some(flat);
                    loaded += 1;
                }
            }
        }

        index.upload(queue);
        coarse.upload(queue);

        let elapsed = start.elapsed();
        log::info!(
            "preload: {loaded} bricks + {air} air in {:.0} ms ({total_to_load} cells)",
            elapsed.as_secs_f64() * 1000.0
        );
    }

    /// Blocks until all grid cells within `radius` (world units) of `center`
    /// are loaded or confirmed air. Cells outside the radius are left for
    /// streaming via [`update()`](Self::update).
    pub fn preload_radius(
        &mut self,
        center: Vec3,
        radius: f32,
        index: &mut BrickIndex,
        pool: &mut BrickPool,
        coarse: &mut CoarseMipGrid,
        queue: &wgpu::Queue,
    ) {
        let start = Instant::now();
        let r2 = radius * radius;
        let mut total_to_load = 0u32;

        if let Some(tx) = &self.request_tx {
            for gz in 0..self.dims[2] {
                for gy in 0..self.dims[1] {
                    for gx in 0..self.dims[0] {
                        let grid_pos = [gx, gy, gz];
                        let flat = self.flat_index(grid_pos);
                        if self.cell_states[flat] != CellState::Unknown {
                            continue;
                        }
                        let cell_center = self.world_min
                            + Vec3::new(
                                (gx as f32 + 0.5) * BRICK_SIZE,
                                (gy as f32 + 0.5) * BRICK_SIZE,
                                (gz as f32 + 0.5) * BRICK_SIZE,
                            );
                        if cell_center.distance_squared(center) > r2 {
                            continue;
                        }
                        tx.send(grid_pos).expect("worker channel closed");
                        self.cell_states[flat] = CellState::Loading {
                            existing_slot: None,
                        };
                        total_to_load += 1;
                    }
                }
            }
        }

        let mut loaded = 0u32;
        let mut air = 0u32;
        for _ in 0..total_to_load {
            let result = self.result_rx.recv().expect("worker channel closed");
            let flat = self.flat_index(result.grid_pos);

            match result.data {
                None => {
                    self.cell_states[flat] = CellState::Air;
                    air += 1;
                }
                Some(data) => {
                    let handle = pool.alloc().expect("pool exhausted during preload_radius");
                    pool.write_voxels(queue, handle, &data.voxels);
                    pool.write_palette(queue, handle, &data.palette);
                    pool.write_mips(queue, handle, &result.mips);
                    coarse.write_cell(result.grid_pos, &result.mips);
                    index.set(result.grid_pos, handle);

                    let slot = handle.gpu_index();
                    self.cell_states[flat] = CellState::Resident { slot };
                    self.slot_last_used[slot as usize] = self.frame;
                    self.slot_cell[slot as usize] = Some(flat);
                    loaded += 1;
                }
            }
        }

        index.upload(queue);
        coarse.upload(queue);

        let elapsed = start.elapsed();
        log::info!(
            "preload_radius({radius:.0}m): {loaded} bricks + {air} air in {:.0} ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }

    /// Runs one frame of the paging loop.
    ///
    /// 1. Drains completed loads and uploads to GPU (capped).
    /// 2. Walks the grid, classifies cells by SSE, submits load requests.
    ///
    /// `focal_length` is `screen_height / (2 * tan(fov_y / 2))` — the same
    /// value the raymarcher uses for SSE computation.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        camera_pos: Vec3,
        focal_length: f32,
        sse_threshold: f32,
        index: &mut BrickIndex,
        pool: &mut BrickPool,
        coarse: &mut CoarseMipGrid,
        queue: &wgpu::Queue,
    ) -> PagerStats {
        self.frame += 1;

        let (uploaded, evicted) = self.drain_results(index, pool, coarse, queue);

        self.compute_demand(camera_pos, focal_length, sse_threshold);

        if self.frame.is_multiple_of(60) {
            let stats = self.tally_stats(uploaded, evicted);
            log::debug!(
                "pager f{}: up={} ev={} res={} mip={} load={} unk={} air={}",
                self.frame,
                uploaded,
                evicted,
                stats.resident,
                stats.mip_only,
                stats.loading,
                stats.unknown,
                stats.air,
            );
            return stats;
        }
        self.tally_stats(uploaded, evicted)
    }

    /// Phase 1: drain completed loads and upload to GPU.
    fn drain_results(
        &mut self,
        index: &mut BrickIndex,
        pool: &mut BrickPool,
        coarse: &mut CoarseMipGrid,
        queue: &wgpu::Queue,
    ) -> (u32, u32) {
        let mut uploaded = 0u32;
        let mut evicted = 0u32;
        let mut index_dirty = false;

        let mut eviction_queue: Vec<(u32, usize)> = Vec::new();
        if pool.live_count() >= pool.capacity() {
            let mut candidates: Vec<(u32, usize, u64)> = Vec::new();
            for (slot, cell_flat) in self.slot_cell.iter().enumerate() {
                if let Some(flat) = cell_flat
                    && matches!(
                        self.cell_states[*flat],
                        CellState::MipOnly { .. } | CellState::Resident { .. }
                    )
                {
                    candidates.push((slot as u32, *flat, self.slot_last_used[slot]));
                }
            }
            candidates.sort_unstable_by_key(|&(_, _, frame)| frame);
            eviction_queue = candidates.into_iter().map(|(s, f, _)| (s, f)).collect();
        }
        let mut evict_idx = 0;

        while uploaded < self.config.max_uploads_per_frame {
            let result = match self.result_rx.try_recv() {
                Ok(r) => r,
                Err(_) => break,
            };

            self.in_flight.remove(&result.grid_pos);
            let flat = self.flat_index(result.grid_pos);
            let existing_slot = match self.cell_states[flat] {
                CellState::Loading { existing_slot } => existing_slot,
                _ => None,
            };

            let data = match result.data {
                Some(d) => d,
                None => {
                    self.cell_states[flat] = CellState::Air;
                    continue;
                }
            };

            let handle = if let Some(slot) = existing_slot {
                pool.reassign(slot)
            } else if let Some(h) = pool.alloc() {
                h
            } else {
                let mut found = None;
                while evict_idx < eviction_queue.len() {
                    let (old_slot, old_flat) = eviction_queue[evict_idx];
                    evict_idx += 1;
                    if matches!(
                        self.cell_states[old_flat],
                        CellState::MipOnly { .. } | CellState::Resident { .. }
                    ) {
                        index.clear_cell(self.unflatten(old_flat));
                        self.cell_states[old_flat] = CellState::Unknown;
                        self.slot_cell[old_slot as usize] = None;
                        evicted += 1;
                        found = Some(pool.reassign(old_slot));
                        break;
                    }
                }
                match found {
                    Some(h) => h,
                    None => break,
                }
            };

            pool.write_voxels(queue, handle, &data.voxels);
            pool.write_palette(queue, handle, &data.palette);
            pool.write_mips(queue, handle, &result.mips);
            coarse.write_cell(result.grid_pos, &result.mips);
            coarse.upload_cell(queue, result.grid_pos);
            index.set(result.grid_pos, handle);
            index_dirty = true;

            let slot = handle.gpu_index();
            self.cell_states[flat] = CellState::Resident { slot };
            self.slot_last_used[slot as usize] = self.frame;
            self.slot_cell[slot as usize] = Some(flat);
            uploaded += 1;
        }

        if index_dirty {
            index.upload(queue);
        }

        (uploaded, evicted)
    }

    /// Phase 2: walk cells near the camera, classify by SSE, submit load requests.
    fn compute_demand(&mut self, camera_pos: Vec3, focal_length: f32, sse_threshold: f32) {
        let mut requests: Vec<([u32; 3], u32)> = Vec::new();

        let cam_grid = (camera_pos - self.world_min) / BRICK_SIZE;
        let demand_radius = 50i32;

        let gx_min = ((cam_grid.x as i32 - demand_radius).max(0) as u32).min(self.dims[0]);
        let gx_max = ((cam_grid.x as i32 + demand_radius).max(0) as u32).min(self.dims[0]);
        let gy_min = 0u32;
        let gy_max = self.dims[1];
        let gz_min = ((cam_grid.z as i32 - demand_radius).max(0) as u32).min(self.dims[2]);
        let gz_max = ((cam_grid.z as i32 + demand_radius).max(0) as u32).min(self.dims[2]);

        for gz in gz_min..gz_max {
            for gy in gy_min..gy_max {
                for gx in gx_min..gx_max {
                    let grid_pos = [gx, gy, gz];
                    let flat = self.flat_index(grid_pos);
                    let state = self.cell_states[flat];

                    let cell_center = self.world_min
                        + Vec3::new(
                            (gx as f32 + 0.5) * BRICK_SIZE,
                            (gy as f32 + 0.5) * BRICK_SIZE,
                            (gz as f32 + 0.5) * BRICK_SIZE,
                        );
                    let dist = cell_center.distance(camera_pos).max(0.01);
                    let sse = BRICK_SIZE * focal_length / dist;

                    if sse >= sse_threshold {
                        match state {
                            CellState::Unknown | CellState::MipOnly { .. } => {
                                if !self.in_flight.contains(&grid_pos) {
                                    requests.push((grid_pos, (sse * 1000.0) as u32));
                                }
                            }
                            CellState::Resident { slot } => {
                                self.slot_last_used[slot as usize] = self.frame;
                            }
                            CellState::Air | CellState::Loading { .. } => {}
                        }
                    } else if let CellState::Resident { slot } = state {
                        self.cell_states[flat] = CellState::MipOnly { slot };
                    }
                }
            }
        }

        requests.sort_unstable_by_key(|r| std::cmp::Reverse(r.1));

        if let Some(tx) = &self.request_tx {
            for (grid_pos, _) in requests {
                let flat = self.flat_index(grid_pos);
                let existing_slot = match self.cell_states[flat] {
                    CellState::MipOnly { slot } => Some(slot),
                    _ => None,
                };
                if tx.try_send(grid_pos).is_ok() {
                    self.in_flight.insert(grid_pos);
                    self.cell_states[flat] = CellState::Loading { existing_slot };
                }
            }
        }
    }

    fn tally_stats(&self, uploaded: u32, evicted: u32) -> PagerStats {
        let mut stats = PagerStats {
            evicted_this_frame: evicted,
            uploaded_this_frame: uploaded,
            ..Default::default()
        };
        for &state in &self.cell_states {
            match state {
                CellState::Unknown => stats.unknown += 1,
                CellState::Air => stats.air += 1,
                CellState::Loading { .. } => stats.loading += 1,
                CellState::Resident { .. } => stats.resident += 1,
                CellState::MipOnly { .. } => stats.mip_only += 1,
            }
        }
        stats
    }

    fn flat_index(&self, pos: [u32; 3]) -> usize {
        (pos[0] + self.dims[0] * (pos[1] + self.dims[1] * pos[2])) as usize
    }

    fn unflatten(&self, flat: usize) -> [u32; 3] {
        let flat = flat as u32;
        let x = flat % self.dims[0];
        let y = (flat / self.dims[0]) % self.dims[1];
        let z = flat / (self.dims[0] * self.dims[1]);
        [x, y, z]
    }

    fn spawn_workers(
        source: Arc<dyn BrickSource>,
        world_min: Vec3,
        request_rx: crossbeam_channel::Receiver<[u32; 3]>,
        result_tx: crossbeam_channel::Sender<LoadResult>,
        count: usize,
    ) -> Vec<Option<thread::JoinHandle<()>>> {
        (0..count)
            .map(|i| {
                let source = Arc::clone(&source);
                let rx = request_rx.clone();
                let tx = result_tx.clone();
                Some(
                    thread::Builder::new()
                        .name(format!("brick-pager-{i}"))
                        .spawn(move || {
                            while let Ok(grid_pos) = rx.recv() {
                                let data = source.generate(grid_pos, world_min);
                                let mips = match &data {
                                    Some(d) => mip::compute_brick_mips(&d.voxels, &d.palette),
                                    None => [0u32; mip::MIP_WORDS_PER_BRICK as usize],
                                };
                                if tx
                                    .send(LoadResult {
                                        grid_pos,
                                        data,
                                        mips,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        })
                        .expect("failed to spawn pager worker"),
                )
            })
            .collect()
    }
}

impl Drop for BrickPager {
    fn drop(&mut self) {
        self.request_tx.take();
        for w in &mut self.workers {
            if let Some(handle) = w.take() {
                let _ = handle.join();
            }
        }
    }
}
