//! Coarse SVO construction from the terrain heightfield.
//!
//! Builds the complete octree *structure* for a world — air pruned, buried
//! volume as solid coarse leaves, the surface band as colored leaf cells —
//! without generating any brick voxel data. The whole world renders at LOD
//! immediately; the pager later attaches real bricks near the camera.

use glam::Vec3;
use smallworld_engine::brick_pool::{BRICK_EDGE, VOXEL_SCALE};
use smallworld_engine::svo::Svo;

use crate::worldgen::WorldGenerator;

const BRICK_SIZE: f32 = BRICK_EDGE as f32 * VOXEL_SCALE;

/// Conservative slack (m) around sampled surface heights: covers density
/// variation between column-center samples and the find_surface_y step size.
const HEIGHT_PAD: f32 = 1.0;

const COL_GRASS: [u8; 4] = [76, 153, 0, 255];
const COL_DIRT: [u8; 4] = [139, 90, 43, 255];
const COL_STONE: [u8; 4] = [128, 128, 128, 255];
const COL_DARK_STONE: [u8; 4] = [80, 80, 90, 255];
const COL_WATER: [u8; 4] = [30, 100, 180, 255];

/// Output of the coarse pass consumed by the streaming pager.
pub struct CoarseWorld {
    /// Per-column `[lo, hi]` inclusive cell band eligible for brick
    /// streaming (surface crossing + lateral exposure). `(1, 0)` = none.
    /// Indexed `x + dims[0] * z`.
    pub column_range: Vec<(u16, u16)>,
}

struct Builder<'a> {
    svo: &'a mut Svo,
    /// Per-level min/max effective-height pyramid over the xz footprint.
    /// `pyramid[l]` has `2^l × 2^l` entries; level 0 is the root.
    pyramid: Vec<Vec<(f32, f32)>>,
    /// Raw per-column surface height (leaf resolution), NEG_INFINITY = none.
    surface: Vec<f32>,
    depth: u32,
    leaf_cells: u32,
    world_min_y: f32,
    water_level: f32,
}

/// Builds the coarse tree for the full world and returns the streaming
/// candidate set. The caller runs `svo.upload()` afterwards.
pub fn build_coarse(svo: &mut Svo, generator: &WorldGenerator, dims: [u32; 3]) -> CoarseWorld {
    let start = std::time::Instant::now();
    let depth = (svo.world_size() / BRICK_SIZE).log2().round() as u32;
    let leaf_cells = 1u32 << depth;
    assert_eq!(
        leaf_cells, dims[0],
        "coarse pass expects the x/z lattice to span the full tree"
    );

    let world_min = svo.world_min();
    let surface = sample_heights(generator, leaf_cells, world_min);

    // Effective height drives occupancy: open water fills up to water level.
    let water = generator.water_level();
    let eff: Vec<f32> = surface
        .iter()
        .map(|&s| if s.is_finite() { s.max(water) } else { water })
        .collect();

    let pyramid = build_pyramid(&eff, depth);

    let mut builder = Builder {
        svo,
        pyramid,
        surface,
        depth,
        leaf_cells,
        world_min_y: world_min.y,
        water_level: water,
    };
    let root = builder.svo.root();
    builder.build_children(root, 0, [0, 0, 0]);
    builder.svo.update_colors();

    let column_range = candidate_ranges(&eff, leaf_cells, dims, world_min.y);

    let elapsed = start.elapsed();
    let candidates: usize = column_range
        .iter()
        .filter(|(lo, hi)| lo <= hi)
        .map(|(lo, hi)| (hi - lo + 1) as usize)
        .sum();
    log::info!(
        "coarse SVO: {} nodes, {candidates} candidate cells in {:.0} ms",
        builder.svo.node_count(),
        elapsed.as_secs_f64() * 1000.0
    );

    CoarseWorld { column_range }
}

/// Samples `find_surface_y` at every leaf-column center across threads.
fn sample_heights(generator: &WorldGenerator, leaf_cells: u32, world_min: Vec3) -> Vec<f32> {
    let n = (leaf_cells * leaf_cells) as usize;
    let mut heights = vec![f32::NEG_INFINITY; n];
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let rows_per = leaf_cells.div_ceil(threads as u32);

    std::thread::scope(|scope| {
        for (chunk_idx, chunk) in heights
            .chunks_mut((rows_per * leaf_cells) as usize)
            .enumerate()
        {
            let z0 = chunk_idx as u32 * rows_per;
            scope.spawn(move || {
                for (i, h) in chunk.iter_mut().enumerate() {
                    let x = i as u32 % leaf_cells;
                    let z = z0 + i as u32 / leaf_cells;
                    let wx = world_min.x + (x as f32 + 0.5) * BRICK_SIZE;
                    let wz = world_min.z + (z as f32 + 0.5) * BRICK_SIZE;
                    if let Some(s) = generator.find_surface_y(wx, wz) {
                        *h = s;
                    }
                }
            });
        }
    });
    heights
}

/// Min/max mip pyramid over effective heights, level 0 (root) to `depth`.
fn build_pyramid(eff: &[f32], depth: u32) -> Vec<Vec<(f32, f32)>> {
    let mut pyramid: Vec<Vec<(f32, f32)>> = Vec::with_capacity(depth as usize + 1);
    let leaf: Vec<(f32, f32)> = eff.iter().map(|&h| (h, h)).collect();
    pyramid.push(leaf);

    let mut size = 1u32 << depth;
    while size > 1 {
        let half = size / 2;
        let prev = pyramid.last().unwrap();
        let mut level = vec![(f32::INFINITY, f32::NEG_INFINITY); (half * half) as usize];
        for z in 0..half {
            for x in 0..half {
                let mut mn = f32::INFINITY;
                let mut mx = f32::NEG_INFINITY;
                for (dz, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let (a, b) = prev[((2 * z + dz) * size + 2 * x + dx) as usize];
                    mn = mn.min(a);
                    mx = mx.max(b);
                }
                level[(z * half + x) as usize] = (mn, mx);
            }
        }
        pyramid.push(level);
        size = half;
    }

    pyramid.reverse(); // level 0 = root
    pyramid
}

impl Builder<'_> {
    fn cell_size(&self, level: u32) -> f32 {
        BRICK_SIZE * (1u32 << (self.depth - level)) as f32
    }

    fn footprint(&self, level: u32, cx: u32, cz: u32) -> (f32, f32) {
        self.pyramid[level as usize][(cz * (1u32 << level) + cx) as usize]
    }

    /// Classifies and builds all children of `parent` (at `parent_cell`,
    /// `level`); children live at `level + 1`.
    fn build_children(&mut self, parent: u32, level: u32, parent_cell: [u32; 3]) {
        let child_level = level + 1;
        let size = self.cell_size(child_level);

        for octant in 0..8u8 {
            let cx = parent_cell[0] * 2 + u32::from(octant & 1);
            let cy = parent_cell[1] * 2 + u32::from((octant >> 1) & 1);
            let cz = parent_cell[2] * 2 + u32::from((octant >> 2) & 1);

            let (min_h, max_h) = self.footprint(child_level, cx, cz);
            let y0 = self.world_min_y + cy as f32 * size;
            let y1 = y0 + size;

            if y0 > max_h + HEIGHT_PAD || min_h == f32::NEG_INFINITY {
                continue; // air above every surface in the footprint
            }

            if y1 <= min_h - HEIGHT_PAD {
                // Entirely buried: solid coarse leaf, no recursion.
                let idx = self.svo.alloc_child(parent, octant);
                let center_y = (y0 + y1) * 0.5;
                let color = if center_y > -5.0 {
                    COL_STONE
                } else {
                    COL_DARK_STONE
                };
                self.svo.set_color(idx, color);
                continue;
            }

            // Crossing the surface band.
            if child_level == self.depth {
                if let Some(color) = self.leaf_color(cx, cy, cz) {
                    let idx = self.svo.alloc_child(parent, octant);
                    self.svo.set_color(idx, color);
                }
            } else {
                let idx = self.svo.alloc_child(parent, octant);
                self.build_children(idx, child_level, [cx, cy, cz]);
            }
        }
    }

    /// Color for a leaf cell in the crossing band, or `None` for air.
    fn leaf_color(&self, cx: u32, cy: u32, cz: u32) -> Option<[u8; 4]> {
        let raw = self.surface[(cz * self.leaf_cells + cx) as usize];
        let eff = if raw.is_finite() {
            raw.max(self.water_level)
        } else {
            self.water_level
        };
        let y0 = self.world_min_y + cy as f32 * BRICK_SIZE;
        let y1 = y0 + BRICK_SIZE;

        if y0 > eff {
            return None; // above the effective surface
        }
        if y1 > eff {
            // Contains the effective surface.
            if raw < self.water_level {
                return Some(COL_WATER);
            }
            return Some(COL_GRASS);
        }
        // Below the surface within the crossing band.
        if raw < self.water_level && y1 > raw {
            return Some(COL_WATER); // water body between raw surface and level
        }
        let depth_below = raw - y1;
        if depth_below < 2.0 {
            Some(COL_DIRT)
        } else if y0 > -5.0 {
            Some(COL_STONE)
        } else {
            Some(COL_DARK_STONE)
        }
    }
}

/// Streaming candidates per column: the surface-crossing band plus lateral
/// exposure (cliff faces visible where a neighbor column's surface is lower;
/// world borders count as fully exposed).
fn candidate_ranges(
    eff: &[f32],
    leaf_cells: u32,
    dims: [u32; 3],
    world_min_y: f32,
) -> Vec<(u16, u16)> {
    let cell_of = |h: f32| -> i64 { ((h - world_min_y) / BRICK_SIZE).floor() as i64 };
    let max_cell = i64::from(dims[1]) - 1;

    let mut ranges = vec![(1u16, 0u16); (leaf_cells * leaf_cells) as usize];
    for z in 0..leaf_cells {
        for x in 0..leaf_cells {
            let own = eff[(z * leaf_cells + x) as usize];
            let mut low = own;
            let mut border = false;
            for (dx, dz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let nx = i64::from(x) + dx;
                let nz = i64::from(z) + dz;
                if nx < 0 || nz < 0 || nx >= i64::from(leaf_cells) || nz >= i64::from(leaf_cells) {
                    border = true;
                } else {
                    low = low.min(eff[(nz as u32 * leaf_cells + nx as u32) as usize]);
                }
            }

            let lo = if border {
                0
            } else {
                (cell_of(low - HEIGHT_PAD)).clamp(0, max_cell)
            };
            let hi = (cell_of(own + HEIGHT_PAD)).clamp(0, max_cell);
            if hi >= lo {
                ranges[(z * leaf_cells + x) as usize] = (lo as u16, hi as u16);
            }
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallworld_engine::gpu::GpuContext;

    fn build_world(cells: u32, dims: [u32; 3], capacity: u32) -> (Svo, CoarseWorld, Vec3) {
        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));
        let world_size = cells as f32 * BRICK_SIZE;
        let half = Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32) * BRICK_SIZE * 0.5;
        let mut svo = Svo::new(&ctx.device, capacity, -half, world_size);
        let generator = WorldGenerator::new(42);
        let coarse = build_coarse(&mut svo, &generator, dims);
        (svo, coarse, -half)
    }

    /// Every column must expose a streaming band that contains the cell
    /// holding its effective surface — a missing band means holes where
    /// bricks can never stream in.
    #[test]
    fn every_column_band_contains_its_surface() {
        let dims = [32u32, 12, 32];
        let (svo, coarse, world_min) = build_world(32, dims, 1_000_000);
        let generator = WorldGenerator::new(42);

        for z in 0..dims[2] {
            for x in 0..dims[0] {
                let (lo, hi) = coarse.column_range[(x + dims[0] * z) as usize];
                assert!(lo <= hi, "column ({x},{z}) has no candidate band");

                let wx = world_min.x + (x as f32 + 0.5) * BRICK_SIZE;
                let wz = world_min.z + (z as f32 + 0.5) * BRICK_SIZE;
                let surface = generator
                    .find_surface_y(wx, wz)
                    .unwrap_or_else(|| generator.water_level())
                    .max(generator.water_level());
                let cell = (((surface - world_min.y) / BRICK_SIZE).floor() as i64)
                    .clamp(0, i64::from(dims[1]) - 1) as u16;
                assert!(
                    cell >= lo && cell <= hi,
                    "column ({x},{z}): surface cell {cell} outside band [{lo},{hi}]"
                );
            }
        }
        assert!(svo.node_count() > 100, "coarse tree suspiciously small");
    }

    /// Regression test for floating coarse boxes: cells the heightfield
    /// painted solid but worldgen reveals as air (cave breaches) must be
    /// cleared during streaming, and the band must extend down to the cave
    /// floor. Invariant: after a full preload, every column's topmost
    /// visible cell (colored or brick-bearing) actually contains solid
    /// voxels per the generator.
    #[test]
    fn preload_leaves_no_floating_boxes() {
        use smallworld_engine::brick_data::BrickData;
        use smallworld_engine::brick_pager::{BrickPager, PagerConfig};
        use smallworld_engine::brick_pool::BrickPool;
        use smallworld_engine::brick_source::BrickSource;
        use std::sync::Arc;

        struct CpuSource(WorldGenerator);
        impl BrickSource for CpuSource {
            fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
                self.0
                    .generate_brick(grid_pos, world_min)
                    .map(|g| BrickData {
                        voxels: g.voxels,
                        palette: g.palette.to_vec(),
                    })
            }
        }

        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));
        let dims = [32u32, 12, 32];
        let world_size = 32.0 * BRICK_SIZE;
        let half = Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32) * BRICK_SIZE * 0.5;
        let world_min = -half;
        let mut svo = Svo::new(&ctx.device, 1_000_000, world_min, world_size);
        let generator = WorldGenerator::new(42);
        let coarse = build_coarse(&mut svo, &generator, dims);

        let mut pool = BrickPool::new(&ctx.device, 32768);
        let mut pager = BrickPager::new(
            Arc::new(CpuSource(WorldGenerator::new(42))),
            dims,
            world_min,
            coarse.column_range,
            32768,
            PagerConfig::default(),
        );
        pager.preload_all(&mut svo, &mut pool, &ctx.queue);

        // Compare end state against the app's HUD numbers (Default preset).
        let stats = pager.update(
            Vec3::new(0.0, 8.0, 14.0),
            623.0,
            0.8,
            &mut svo,
            &mut pool,
            &ctx.queue,
        );
        eprintln!(
            "pager end state: resident={} air={} unknown={} loading={}",
            stats.resident, stats.air, stats.unknown, stats.loading
        );

        // Ground-truth occupancy for every cell: 0 = air, 1 = partial
        // (has air voxels), 2 = full solid. Generated across threads.
        let n_cells = (dims[0] * dims[1] * dims[2]) as usize;
        let mut occ = vec![0u8; n_cells];
        let per_slice = (dims[0] * dims[1]) as usize;
        std::thread::scope(|scope| {
            for (z, chunk) in occ.chunks_mut(per_slice).enumerate() {
                scope.spawn(move || {
                    let generator = WorldGenerator::new(42);
                    for (i, cell) in chunk.iter_mut().enumerate() {
                        let x = i as u32 % dims[0];
                        let y = i as u32 / dims[0];
                        *cell = match generator.generate_brick([x, y, z as u32], world_min) {
                            None => 0,
                            Some(g) if g.voxels.iter().all(|&v| v != 0) => 2,
                            Some(_) => 1,
                        };
                    }
                });
            }
        });
        let occupancy = |x: i64, y: i64, z: i64| -> Option<u8> {
            (x >= 0
                && y >= 0
                && z >= 0
                && x < i64::from(dims[0])
                && y < i64::from(dims[1])
                && z < i64::from(dims[2]))
            .then(|| occ[(x + i64::from(dims[0]) * (y + i64::from(dims[1]) * z)) as usize])
        };
        let is_solid =
            |x: i64, y: i64, z: i64| -> Option<bool> { occupancy(x, y, z).map(|o| o != 0) };

        let mut floating = 0u32;
        let mut coarse_faces = 0u32;
        for z in 0..dims[2] {
            for x in 0..dims[0] {
                for y in 0..dims[1] {
                    let pos = world_min + Vec3::new(x as f32, y as f32, z as f32) * BRICK_SIZE;
                    let info = svo.leaf_info(pos, BRICK_SIZE);
                    let visible =
                        info.is_some_and(|(color, brick)| brick || (color >> 24) & 0xFF > 0);
                    let cell_solid =
                        is_solid(i64::from(x), i64::from(y), i64::from(z)) == Some(true);

                    // Rule 1: no opaque cell where the generator says air,
                    // unless a solid cell above hides it anyway.
                    if visible && !cell_solid {
                        let occluded =
                            is_solid(i64::from(x), i64::from(y) + 1, i64::from(z)) == Some(true);
                        if !occluded {
                            floating += 1;
                            eprintln!("floating box at ({x},{y},{z})");
                        }
                    }

                    // Rule 2: every exposed solid cell must carry brick
                    // detail. Exposure is voxel-granular: a neighbor with
                    // ANY air voxels (fully air OR partial, e.g. the water
                    // surface cell) reveals this cell's face — cell-level
                    // "air neighbor" checks miss the waterline wall class.
                    if cell_solid {
                        let exposed = [
                            (-1i64, 0i64, 0i64),
                            (1, 0, 0),
                            (0, -1, 0),
                            (0, 1, 0),
                            (0, 0, -1),
                            (0, 0, 1),
                        ]
                        .iter()
                        .any(|(dx, dy, dz)| {
                            matches!(
                                occupancy(i64::from(x) + dx, i64::from(y) + dy, i64::from(z) + dz),
                                Some(0) | Some(1)
                            )
                        });
                        if exposed && !info.is_some_and(|(_, brick)| brick) {
                            coarse_faces += 1;
                            eprintln!("coarse exposed face at ({x},{y},{z})");
                        }
                    }
                }
            }
        }
        assert_eq!(floating, 0, "{floating} opaque cells over air");
        assert_eq!(
            coarse_faces, 0,
            "{coarse_faces} exposed solid cells lack brick detail"
        );
    }

    /// AC probe for the 1 km world: coarse construction under 5 s.
    /// Heavy (1024² columns) — run explicitly: `cargo test --release
    /// -p smallworld-sandbox large_world -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn large_world_coarse_build_under_5s() {
        let start = std::time::Instant::now();
        let (svo, coarse, _) = build_world(1024, [1024, 16, 1024], 8_000_000);
        let elapsed = start.elapsed();

        let candidates: usize = coarse
            .column_range
            .iter()
            .filter(|(lo, hi)| lo <= hi)
            .map(|(lo, hi)| (hi - lo + 1) as usize)
            .sum();
        println!(
            "large world coarse build: {:.2} s, {} nodes, {candidates} candidates",
            elapsed.as_secs_f64(),
            svo.node_count()
        );
        assert!(
            elapsed.as_secs_f64() < 5.0,
            "coarse build took {:.2} s (AC: < 5 s)",
            elapsed.as_secs_f64()
        );
    }
}

#[cfg(test)]
mod render_probe {
    use super::*;
    use crate::worldgen::WorldGenerator;
    // CpuSource is shared with gpu_bench.
    use smallworld_engine::brick_data::BrickData;
    use smallworld_engine::brick_pager::{BrickPager, PagerConfig};
    use smallworld_engine::brick_pool::BrickPool;
    use smallworld_engine::brick_source::BrickSource;
    use smallworld_engine::camera::FreeCamera;
    use smallworld_engine::gpu::GpuContext;
    use smallworld_engine::raymarcher::Raymarcher;
    use smallworld_engine::wgpu;
    use smallworld_engine::world::World;
    use std::sync::Arc;

    struct CpuSource(WorldGenerator);
    impl BrickSource for CpuSource {
        fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
            self.0
                .generate_brick(grid_pos, world_min)
                .map(|g| BrickData {
                    voxels: g.voxels,
                    palette: g.palette.to_vec(),
                })
        }
    }

    /// Diagnostic: renders the Default world headlessly with the exact
    /// camera from the user's screenshot and writes a PPM for inspection.
    /// `BENCH_WORLD=large` renders the 1 km world at the bench viewpoint
    /// instead — the same setup as `bench_raymarch`, for A/B image compares.
    /// Run: cargo test -p smallworld-sandbox render_default_headless -- --ignored --nocapture
    #[test]
    #[ignore]
    fn render_default_headless() {
        let large = std::env::var("BENCH_WORLD").as_deref() == Ok("large");
        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));
        let (dims, cells, capacity) = if large {
            ([1024u32, 16, 1024], 1024u32, 8_000_000u32)
        } else {
            ([32u32, 12, 32], 32, 1_000_000)
        };
        let world_size = cells as f32 * BRICK_SIZE;
        let half = Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32) * BRICK_SIZE * 0.5;
        let world_min = -half;
        let mut svo = Svo::new(&ctx.device, capacity, world_min, world_size);
        let generator = WorldGenerator::new(42);
        let coarse = build_coarse(&mut svo, &generator, dims);

        let mut pool = BrickPool::new(&ctx.device, 32768);
        let mut pager = BrickPager::new(
            Arc::new(CpuSource(WorldGenerator::new(42))),
            dims,
            world_min,
            coarse.column_range,
            32768,
            PagerConfig {
                worker_threads: 8,
                ..PagerConfig::default()
            },
        );
        let camera_pos = if large {
            Vec3::new(-2.0, 20.1, 16.8)
        } else {
            Vec3::new(-4.0, 5.0, 9.6)
        };
        if large {
            pager.preload_radius(camera_pos, 60.0, &mut svo, &mut pool, &ctx.queue);
        } else {
            pager.preload_all(&mut svo, &mut pool, &ctx.queue);
        }

        let mut world = World::new(&ctx.device);
        let world_data = world.extract();

        let (w, h) = (1280u32, 720u32);
        let mut raymarcher = Raymarcher::new(
            &ctx,
            w,
            h,
            wgpu::TextureFormat::Rgba8Unorm,
            &pool,
            &svo,
            world_data,
        );
        raymarcher.set_terrain_top_y(world_min.y + dims[1] as f32 * BRICK_SIZE);

        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe_target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut camera = FreeCamera::new(w as f32 / h as f32);
        camera.position = camera_pos;
        if large {
            camera.yaw = (4.1_f32).to_radians();
            camera.pitch = (-32.7_f32).to_radians();
        } else {
            camera.yaw = (-6.0_f32).to_radians();
            camera.pitch = (-30.3_f32).to_radians();
        }

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        raymarcher.render(
            &ctx,
            &mut encoder,
            &target_view,
            &camera,
            &svo,
            world_data,
            Raymarcher::FLAG_SHADOWS,
            0.8,
            None,
            None,
        );

        let bytes_per_row = w * 4; // 5120, multiple of 256
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe_readback"),
            size: u64::from(bytes_per_row * h),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit(std::iter::once(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();

        let view = readback.slice(..).get_mapped_range().expect("mapped");
        let out =
            std::env::var("PROBE_OUT").unwrap_or_else(|_| "/tmp/smallworld_probe.ppm".to_string());
        let mut ppm = Vec::with_capacity((w * h * 3) as usize + 32);
        ppm.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
        for px in view.chunks(4) {
            ppm.extend_from_slice(&px[..3]);
        }
        std::fs::write(&out, &ppm).unwrap();
        eprintln!("wrote {out}");
    }
}

#[cfg(test)]
mod gpu_bench {
    use super::*;
    use crate::worldgen::WorldGenerator;
    use smallworld_engine::brick_data::BrickData;
    use smallworld_engine::brick_pager::{BrickPager, PagerConfig};
    use smallworld_engine::brick_pool::BrickPool;
    use smallworld_engine::brick_source::BrickSource;
    use smallworld_engine::camera::FreeCamera;
    use smallworld_engine::gpu::GpuContext;
    use smallworld_engine::raymarcher::Raymarcher;
    use smallworld_engine::wgpu;
    use smallworld_engine::world::World;
    use std::sync::Arc;

    struct CpuSource(WorldGenerator);
    impl BrickSource for CpuSource {
        fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
            self.0
                .generate_brick(grid_pos, world_min)
                .map(|g| BrickData {
                    voxels: g.voxels,
                    palette: g.palette.to_vec(),
                })
        }
    }

    /// Wall-clocks the raymarch compute pass headlessly (median of 30 after
    /// 5 warmup frames), shadows on and off. Nothing else runs on the GPU,
    /// so wall time ≈ GPU time. `BENCH_WORLD=large` benches the 1 km world
    /// at the user's measured viewpoint; default is the Default preset.
    /// Run: cargo test --release -p smallworld-sandbox bench_raymarch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_raymarch() {
        let large = std::env::var("BENCH_WORLD").as_deref() == Ok("large");
        let instance = GpuContext::create_instance();
        let ctx = pollster::block_on(GpuContext::headless(instance));

        let (dims, cells, capacity) = if large {
            ([1024u32, 16, 1024], 1024u32, 8_000_000u32)
        } else {
            ([32u32, 12, 32], 32, 1_000_000)
        };
        let world_size = cells as f32 * BRICK_SIZE;
        let half = Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32) * BRICK_SIZE * 0.5;
        let world_min = -half;
        let mut svo = Svo::new(&ctx.device, capacity, world_min, world_size);
        let generator = WorldGenerator::new(42);
        let coarse = build_coarse(&mut svo, &generator, dims);

        let mut pool = BrickPool::new(&ctx.device, 32768);
        let mut pager = BrickPager::new(
            Arc::new(CpuSource(WorldGenerator::new(42))),
            dims,
            world_min,
            coarse.column_range,
            32768,
            PagerConfig {
                worker_threads: 8,
                ..PagerConfig::default()
            },
        );
        let camera_pos = if large {
            Vec3::new(-2.0, 20.1, 16.8)
        } else {
            Vec3::new(-4.0, 5.0, 9.6)
        };
        if large {
            pager.preload_radius(camera_pos, 60.0, &mut svo, &mut pool, &ctx.queue);
        } else {
            pager.preload_all(&mut svo, &mut pool, &ctx.queue);
        }

        let mut world = World::new(&ctx.device);
        let world_data = world.extract();

        let (w, h) = (1280u32, 720u32);
        let mut raymarcher = Raymarcher::new(
            &ctx,
            w,
            h,
            wgpu::TextureFormat::Rgba8Unorm,
            &pool,
            &svo,
            world_data,
        );
        raymarcher.set_terrain_top_y(world_min.y + dims[1] as f32 * BRICK_SIZE);

        let mut camera = FreeCamera::new(w as f32 / h as f32);
        camera.position = camera_pos;
        if large {
            camera.yaw = (4.1_f32).to_radians();
            camera.pitch = (-32.7_f32).to_radians();
        } else {
            camera.yaw = (-6.0_f32).to_radians();
            camera.pitch = (-30.3_f32).to_radians();
        }

        let sse: f32 = std::env::var("BENCH_SSE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.8);
        for &flags in &[Raymarcher::FLAG_SHADOWS, 0u32] {
            let mut samples = Vec::new();
            for i in 0..35 {
                let mut encoder = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                raymarcher.compute_pass(
                    &ctx,
                    &mut encoder,
                    &camera,
                    &svo,
                    world_data,
                    flags,
                    sse,
                    None,
                );
                let start = std::time::Instant::now();
                ctx.queue.submit(std::iter::once(encoder.finish()));
                let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                if i >= 5 {
                    samples.push(ms);
                }
            }
            samples.sort_by(f64::total_cmp);
            let label = if flags != 0 {
                "shadows ON "
            } else {
                "shadows OFF"
            };
            println!(
                "{} sse={sse} {}: median {:.2} ms  (p10 {:.2}, p90 {:.2})",
                if large { "large" } else { "default" },
                label,
                samples[samples.len() / 2],
                samples[3],
                samples[samples.len() - 4]
            );
        }
    }
}
