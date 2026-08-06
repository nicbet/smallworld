//! Async brick pager: streams brick data from a [`BrickSource`] into the GPU
//! on demand, with residency tracking and LRU eviction under a hard VRAM budget.
//!
//! The pager only tracks *candidate* cells — the surface-crossing band each
//! column exposes (produced by the coarse SVO pass). Everything else is
//! either air or buried volume that renders from coarse SVO colors, so
//! per-frame cost scales with the streamed working set, not world size.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use glam::Vec3;

use crate::brick_data::BrickData;
use crate::brick_pool::{BRICK_EDGE, BrickPool, VOXEL_SCALE};
use crate::brick_source::BrickSource;
use crate::svo::Svo;

/// World-space edge length of one brick.
const BRICK_SIZE: f32 = BRICK_EDGE as f32 * VOXEL_SCALE;

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
    /// Distant (SSE below threshold); slot valid but eviction candidate.
    MipOnly { slot: u32 },
}

/// Pager configuration.
pub struct PagerConfig {
    /// Maximum brick uploads per frame (default 128).
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
    /// Candidate cells not yet loaded.
    pub unknown: u32,
    /// Candidate cells confirmed empty (air).
    pub air: u32,
    /// Bricks evicted this frame.
    pub evicted_this_frame: u32,
    /// Bricks uploaded to GPU this frame.
    pub uploaded_this_frame: u32,
}

#[derive(Clone, Copy, Default)]
struct StateCounts {
    resident: u32,
    mip_only: u32,
    loading: u32,
    air: u32,
}

struct LoadResult {
    grid_pos: [u32; 3],
    data: Option<BrickData>,
}

/// Async brick pager: manages residency, background loading, and LRU eviction.
///
/// Call [`update()`](Self::update) once per frame. It drains completed loads,
/// uploads bricks to the GPU, computes demand from the camera, and dispatches
/// new load requests to worker threads.
pub struct BrickPager {
    /// Non-Unknown states of candidate cells, keyed by flat index.
    states: HashMap<usize, CellState>,
    /// Per-(x,z)-column inclusive candidate cell band; `(1, 0)` = none.
    column_range: Vec<(u16, u16)>,
    total_candidates: u32,
    counts: StateCounts,
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
    /// Creates a new pager over the given candidate set and spawns background
    /// worker threads. `column_range` holds the per-(x,z)-column inclusive
    /// cell band eligible for streaming, indexed `x + dims[0] * z`.
    pub fn new(
        source: Arc<dyn BrickSource>,
        dims: [u32; 3],
        world_min: Vec3,
        column_range: Vec<(u16, u16)>,
        pool_capacity: u32,
        config: PagerConfig,
    ) -> Self {
        assert_eq!(column_range.len(), (dims[0] * dims[2]) as usize);
        let total_candidates: u32 = column_range
            .iter()
            .filter(|(lo, hi)| lo <= hi)
            .map(|(lo, hi)| u32::from(hi - lo) + 1)
            .sum();

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
            "brick pager: {total_candidates} candidate cells, {} workers, max {} uploads/frame",
            config.worker_threads,
            config.max_uploads_per_frame,
        );

        Self {
            states: HashMap::new(),
            column_range,
            total_candidates,
            counts: StateCounts::default(),
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

    /// The candidate cell band of column (gx, gz), or `None`.
    fn band(&self, gx: u32, gz: u32) -> Option<(u32, u32)> {
        let (lo, hi) = self.column_range[(gx + self.dims[0] * gz) as usize];
        (lo <= hi).then_some((u32::from(lo), u32::from(hi)))
    }

    /// Whether a brick is completely solid — no air voxels at all. Only
    /// fully solid bricks stop the candidate flood: a brick with any air
    /// exposes its neighbors' faces through that air (the water surface
    /// cell revealing the waterline wall behind it is the canonical case).
    fn is_full(data: &BrickData) -> bool {
        data.voxels.iter().all(|&v| v != 0)
    }

    /// Grows candidate bands to cover the exposure surface around a cell
    /// discovered to contain air (fully air or a partial brick): each of
    /// its 6 neighbors becomes a candidate (bands are contiguous, so cells
    /// between a neighbor and the old band edge join too). Cave walls,
    /// floors, ceilings, and waterline walls live outside the heightfield
    /// estimate — this is how they get voxel detail. The flood is bounded
    /// by the exposure shell: fully solid results stop it. Returns the
    /// newly added cells.
    fn extend_bands_around(&mut self, pos: [u32; 3]) -> Vec<[u32; 3]> {
        const DELTAS: [[i64; 3]; 6] = [
            [-1, 0, 0],
            [1, 0, 0],
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
        ];
        let mut added = Vec::new();
        for d in DELTAS {
            let nx = i64::from(pos[0]) + d[0];
            let ny = i64::from(pos[1]) + d[1];
            let nz = i64::from(pos[2]) + d[2];
            if nx < 0
                || ny < 0
                || nz < 0
                || nx >= i64::from(self.dims[0])
                || ny >= i64::from(self.dims[1])
                || nz >= i64::from(self.dims[2])
            {
                continue;
            }
            let (nx, ny, nz) = (nx as u32, ny as u32, nz as u32);
            let col = (nx + self.dims[0] * nz) as usize;
            let (lo, hi) = self.column_range[col];
            let ny16 = ny as u16;
            let (new_lo, new_hi) = if lo > hi {
                (ny16, ny16)
            } else if ny16 < lo {
                (ny16, hi)
            } else if ny16 > hi {
                (lo, ny16)
            } else {
                continue; // already a candidate
            };
            for y in u32::from(new_lo)..=u32::from(new_hi) {
                if lo <= hi && y >= u32::from(lo) && y <= u32::from(hi) {
                    continue; // was already in the band
                }
                let cell = [nx, y, nz];
                self.total_candidates += 1;
                if self.state(self.flat_index(cell)) == CellState::Unknown {
                    added.push(cell);
                }
            }
            self.column_range[col] = (new_lo, new_hi);
        }
        added
    }

    fn state(&self, flat: usize) -> CellState {
        self.states
            .get(&flat)
            .copied()
            .unwrap_or(CellState::Unknown)
    }

    fn set_state(&mut self, flat: usize, new: CellState) {
        let old = self.state(flat);
        for (state, delta) in [(old, -1i64), (new, 1)] {
            let c = &mut self.counts;
            let counter = match state {
                CellState::Unknown => None,
                CellState::Air => Some(&mut c.air),
                CellState::Loading { .. } => Some(&mut c.loading),
                CellState::Resident { .. } => Some(&mut c.resident),
                CellState::MipOnly { .. } => Some(&mut c.mip_only),
            };
            if let Some(counter) = counter {
                *counter = counter.wrapping_add_signed(delta as i32);
            }
        }
        if matches!(new, CellState::Unknown) {
            self.states.remove(&flat);
        } else {
            self.states.insert(flat, new);
        }
    }

    /// Blocks until every candidate cell has been loaded or confirmed air.
    pub fn preload_all(&mut self, svo: &mut Svo, pool: &mut BrickPool, queue: &wgpu::Queue) {
        self.preload_where(svo, pool, queue, f32::INFINITY, Vec3::ZERO);
    }

    /// Blocks until all candidate cells within `radius` (world units) of
    /// `center` are loaded or confirmed air. The rest streams via
    /// [`update()`](Self::update).
    pub fn preload_radius(
        &mut self,
        center: Vec3,
        radius: f32,
        svo: &mut Svo,
        pool: &mut BrickPool,
        queue: &wgpu::Queue,
    ) {
        self.preload_where(svo, pool, queue, radius, center);
    }

    fn preload_where(
        &mut self,
        svo: &mut Svo,
        pool: &mut BrickPool,
        queue: &wgpu::Queue,
        radius: f32,
        center: Vec3,
    ) {
        let start = Instant::now();
        let r2 = radius * radius;
        let mut total_to_load = 0u32;

        if self.request_tx.is_some() {
            for gz in 0..self.dims[2] {
                for gx in 0..self.dims[0] {
                    let Some((lo, hi)) = self.band(gx, gz) else {
                        continue;
                    };
                    for gy in lo..=hi {
                        let grid_pos = [gx, gy, gz];
                        let flat = self.flat_index(grid_pos);
                        if self.state(flat) != CellState::Unknown {
                            continue;
                        }
                        if radius.is_finite()
                            && self.cell_center(grid_pos).distance_squared(center) > r2
                        {
                            continue;
                        }
                        self.request_tx
                            .as_ref()
                            .unwrap()
                            .send(grid_pos)
                            .expect("worker channel closed");
                        self.set_state(
                            flat,
                            CellState::Loading {
                                existing_slot: None,
                            },
                        );
                        total_to_load += 1;
                    }
                }
            }
        }

        let mut loaded = 0u32;
        let mut air = 0u32;
        let mut pending = total_to_load;
        while pending > 0 {
            let result = self.result_rx.recv().expect("worker channel closed");
            pending -= 1;
            let flat = self.flat_index(result.grid_pos);

            match result.data {
                None => {
                    svo.clear_leaf(self.cell_min(result.grid_pos), BRICK_SIZE);
                    self.set_state(flat, CellState::Air);
                    air += 1;
                    // Cave breach: this air cell's walls/floor/ceiling are
                    // the visible surface — pull them into the candidate
                    // set and keep flooding until fully solid results stop
                    // it. Outside the preload radius the cells stay Unknown
                    // and stream later on demand, so a world-spanning cave
                    // system cannot stall startup.
                    for cell in self.extend_bands_around(result.grid_pos) {
                        if radius.is_finite()
                            && self.cell_center(cell).distance_squared(center) > r2
                        {
                            continue;
                        }
                        self.request_tx
                            .as_ref()
                            .unwrap()
                            .send(cell)
                            .expect("worker channel closed");
                        self.set_state(
                            self.flat_index(cell),
                            CellState::Loading {
                                existing_slot: None,
                            },
                        );
                        pending += 1;
                    }
                }
                Some(data) => {
                    let handle = pool.alloc().expect("pool exhausted during preload");
                    pool.write_voxels(queue, handle, &data.voxels);
                    pool.write_palette(queue, handle, &data.palette);

                    let world_pos = self.cell_min(result.grid_pos);
                    let avg_color = brick_avg_color(&data);
                    let full = Self::is_full(&data);
                    svo.insert_brick(world_pos, BRICK_SIZE, handle, avg_color);

                    let slot = handle.gpu_index();
                    self.set_state(flat, CellState::Resident { slot });
                    self.slot_last_used[slot as usize] = self.frame;
                    self.slot_cell[slot as usize] = Some(flat);
                    loaded += 1;

                    // A partial brick exposes its neighbors through its air
                    // voxels — same flood rule as fully-air cells.
                    if !full {
                        for cell in self.extend_bands_around(result.grid_pos) {
                            if radius.is_finite()
                                && self.cell_center(cell).distance_squared(center) > r2
                            {
                                continue;
                            }
                            self.request_tx
                                .as_ref()
                                .unwrap()
                                .send(cell)
                                .expect("worker channel closed");
                            self.set_state(
                                self.flat_index(cell),
                                CellState::Loading {
                                    existing_slot: None,
                                },
                            );
                            pending += 1;
                        }
                    }
                }
            }
        }

        svo.upload(queue);

        let elapsed = start.elapsed();
        log::info!(
            "preload: {loaded} bricks + {air} air in {:.0} ms ({total_to_load} cells)",
            elapsed.as_secs_f64() * 1000.0
        );
    }

    /// Runs one frame of the paging loop.
    ///
    /// 1. Drains completed loads and uploads to GPU (capped).
    /// 2. Walks candidate columns near the camera, classifies by SSE,
    ///    submits load requests.
    ///
    /// `focal_length` is `screen_height / (2 * tan(fov_y / 2))` — the same
    /// value the raymarcher uses for SSE computation.
    pub fn update(
        &mut self,
        camera_pos: Vec3,
        focal_length: f32,
        sse_threshold: f32,
        svo: &mut Svo,
        pool: &mut BrickPool,
        queue: &wgpu::Queue,
    ) -> PagerStats {
        self.frame += 1;

        let (uploaded, evicted) = self.drain_results(svo, pool, queue);

        self.compute_demand(camera_pos, focal_length, sse_threshold);

        let stats = self.tally_stats(uploaded, evicted);
        if self.frame.is_multiple_of(60) {
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
        }
        stats
    }

    /// Phase 1: drain completed loads and upload to GPU.
    fn drain_results(
        &mut self,
        svo: &mut Svo,
        pool: &mut BrickPool,
        queue: &wgpu::Queue,
    ) -> (u32, u32) {
        let mut uploaded = 0u32;
        let mut evicted = 0u32;

        let mut eviction_queue: Vec<(u32, usize)> = Vec::new();
        if pool.live_count() >= pool.capacity() {
            let mut candidates: Vec<(u32, usize, u64)> = Vec::new();
            for (slot, cell_flat) in self.slot_cell.iter().enumerate() {
                if let Some(flat) = cell_flat
                    && matches!(
                        self.state(*flat),
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
            let existing_slot = match self.state(flat) {
                CellState::Loading { existing_slot } => existing_slot,
                _ => None,
            };

            let data = match result.data {
                Some(d) => d,
                None => {
                    svo.clear_leaf(self.cell_min(result.grid_pos), BRICK_SIZE);
                    self.set_state(flat, CellState::Air);
                    // Cave breach: grow the candidate set around the air
                    // cell; the next demand pass requests the new cells.
                    self.extend_bands_around(result.grid_pos);
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
                        self.state(old_flat),
                        CellState::MipOnly { .. } | CellState::Resident { .. }
                    ) {
                        // Detach the brick from the SVO leaf: the color stays
                        // for LOD, but the handle must go — the slot is about
                        // to hold a different cell's voxels (sw-fcea39).
                        let old_pos = self.unflatten(old_flat);
                        svo.remove_brick(self.cell_min(old_pos), BRICK_SIZE);
                        self.set_state(old_flat, CellState::Unknown);
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

            let avg_color = brick_avg_color(&data);
            svo.insert_brick(
                self.cell_min(result.grid_pos),
                BRICK_SIZE,
                handle,
                avg_color,
            );

            let slot = handle.gpu_index();
            self.set_state(flat, CellState::Resident { slot });
            self.slot_last_used[slot as usize] = self.frame;
            self.slot_cell[slot as usize] = Some(flat);
            uploaded += 1;
        }

        svo.upload_dirty(queue);

        (uploaded, evicted)
    }

    /// Phase 2: walk candidate columns near the camera, classify by SSE,
    /// submit load requests.
    fn compute_demand(&mut self, camera_pos: Vec3, focal_length: f32, sse_threshold: f32) {
        let mut requests: Vec<([u32; 3], u32)> = Vec::new();
        let mut transitions: Vec<(usize, CellState)> = Vec::new();

        let cam_grid = (camera_pos - self.world_min) / BRICK_SIZE;
        let demand_radius = 50i32;

        let gx_min = ((cam_grid.x as i32 - demand_radius).max(0) as u32).min(self.dims[0]);
        let gx_max = ((cam_grid.x as i32 + demand_radius).max(0) as u32).min(self.dims[0]);
        let gz_min = ((cam_grid.z as i32 - demand_radius).max(0) as u32).min(self.dims[2]);
        let gz_max = ((cam_grid.z as i32 + demand_radius).max(0) as u32).min(self.dims[2]);

        for gz in gz_min..gz_max {
            for gx in gx_min..gx_max {
                let Some((lo, hi)) = self.band(gx, gz) else {
                    continue;
                };
                for gy in lo..=hi {
                    let grid_pos = [gx, gy, gz];
                    let flat = self.flat_index(grid_pos);
                    let state = self.state(flat);

                    let dist = self.cell_center(grid_pos).distance(camera_pos).max(0.01);
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
                        transitions.push((flat, CellState::MipOnly { slot }));
                    }
                }
            }
        }

        for (flat, state) in transitions {
            self.set_state(flat, state);
        }

        requests.sort_unstable_by_key(|r| std::cmp::Reverse(r.1));

        if self.request_tx.is_some() {
            for (grid_pos, _) in requests {
                let flat = self.flat_index(grid_pos);
                let existing_slot = match self.state(flat) {
                    CellState::MipOnly { slot } => Some(slot),
                    _ => None,
                };
                if self.request_tx.as_ref().unwrap().try_send(grid_pos).is_ok() {
                    self.in_flight.insert(grid_pos);
                    self.set_state(flat, CellState::Loading { existing_slot });
                }
            }
        }
    }

    fn tally_stats(&self, uploaded: u32, evicted: u32) -> PagerStats {
        let c = self.counts;
        PagerStats {
            resident: c.resident,
            mip_only: c.mip_only,
            loading: c.loading,
            unknown: self
                .total_candidates
                .saturating_sub(c.resident + c.mip_only + c.loading + c.air),
            air: c.air,
            evicted_this_frame: evicted,
            uploaded_this_frame: uploaded,
        }
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

    fn cell_min(&self, pos: [u32; 3]) -> Vec3 {
        self.world_min + Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32) * BRICK_SIZE
    }

    fn cell_center(&self, pos: [u32; 3]) -> Vec3 {
        self.cell_min(pos) + Vec3::splat(BRICK_SIZE * 0.5)
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
                                if tx.send(LoadResult { grid_pos, data }).is_err() {
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

/// Computes a quick average color from a brick's voxel + palette data.
fn brick_avg_color(data: &BrickData) -> [u8; 4] {
    let mut r_sum = 0u32;
    let mut g_sum = 0u32;
    let mut b_sum = 0u32;
    let mut count = 0u32;
    for &idx in &data.voxels {
        if idx == 0 {
            continue; // air
        }
        if let Some(color) = data.palette.get(idx as usize) {
            if color[3] == 0 {
                continue;
            }
            r_sum += color[0] as u32;
            g_sum += color[1] as u32;
            b_sum += color[2] as u32;
            count += 1;
        }
    }
    if count == 0 {
        return [128, 128, 128, 255];
    }
    [
        (r_sum / count) as u8,
        (g_sum / count) as u8,
        (b_sum / count) as u8,
        255,
    ]
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
