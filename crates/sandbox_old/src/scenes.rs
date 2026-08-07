//! Scene presets for the sandbox.

use std::sync::Arc;
use std::time::Instant;

use smallworld_engine::brick_pager::{BrickPager, PagerConfig};
use smallworld_engine::brick_pool::{BRICK_EDGE, BRICK_VOLUME, BrickPool, VOXEL_SCALE};
use smallworld_engine::svo::Svo;
use smallworld_engine::voxel_object::VoxelInstance;
use smallworld_engine::wgpu;
use smallworld_engine::world::World;

use crate::cached_source;
use crate::coarse_svo;
use crate::gpu_cached_source::GpuCachedSource;
use crate::gpu_worldgen::GpuWorldGenerator;
use crate::model_gen;
use crate::worldgen::{self, WorldGenerator};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Default,
    TerrainOnly,
    LargeWorld,
    ObjectsOnly,
    Stress,
    SingleBrick,
    Empty,
}

impl Preset {
    pub const ALL: &[Self] = &[
        Self::Default,
        Self::TerrainOnly,
        Self::LargeWorld,
        Self::ObjectsOnly,
        Self::Stress,
        Self::SingleBrick,
        Self::Empty,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::TerrainOnly => "Terrain Only",
            Self::LargeWorld => "Large World",
            Self::ObjectsOnly => "Objects Only",
            Self::Stress => "Stress",
            Self::SingleBrick => "Single Brick",
            Self::Empty => "Empty",
        }
    }

    pub fn grid_dims(self) -> [u32; 3] {
        match self {
            Self::Default | Self::TerrainOnly => [32, 12, 32],
            Self::LargeWorld => [1024, 16, 1024],
            Self::ObjectsOnly | Self::Stress => [4, 2, 4],
            Self::SingleBrick | Self::Empty => [2, 2, 2],
        }
    }

    /// SVO node pool size. The large world needs room for the full coarse
    /// tree (~3-4M nodes estimated); small presets stay lean.
    pub fn svo_capacity(self) -> u32 {
        match self {
            Self::LargeWorld => 8_000_000,
            _ => 1_000_000,
        }
    }

    pub fn world_min(self) -> glam::Vec3 {
        let dims = self.grid_dims();
        let half = glam::Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32)
            * (BRICK_EDGE as f32 * VOXEL_SCALE)
            * 0.5;
        -half
    }

    pub fn pool_capacity(self) -> u32 {
        match self {
            Self::Default | Self::TerrainOnly | Self::LargeWorld => 32768,
            Self::Stress => 16384,
            Self::ObjectsOnly => 8192,
            Self::SingleBrick | Self::Empty => 256,
        }
    }

    pub fn camera_start(self) -> (glam::Vec3, f32, f32) {
        match self {
            Self::Default | Self::TerrainOnly => {
                (glam::Vec3::new(0.0, 8.0, 14.0), 0.0, -20.0_f32.to_radians())
            }
            Self::LargeWorld => (
                glam::Vec3::new(0.0, 30.0, 40.0),
                0.0,
                -20.0_f32.to_radians(),
            ),
            Self::ObjectsOnly => (glam::Vec3::new(0.0, 4.0, 10.0), 0.0, -15.0_f32.to_radians()),
            Self::Stress => (
                glam::Vec3::new(0.0, 20.0, 40.0),
                0.0,
                -25.0_f32.to_radians(),
            ),
            Self::SingleBrick => (glam::Vec3::new(0.0, 1.0, 3.0), 0.0, -10.0_f32.to_radians()),
            Self::Empty => (glam::Vec3::new(0.0, 2.0, 5.0), 0.0, 0.0),
        }
    }

    pub fn camera_path(self, t: f32) -> (glam::Vec3, f32, f32) {
        use std::f32::consts::TAU;
        let t = t.fract();
        match self {
            Self::Default => {
                let angle = t * TAU;
                let r = 18.0;
                let height = 6.0 + 4.0 * (t * TAU * 2.0).sin();
                let x = r * angle.cos();
                let z = r * angle.sin();
                let pitch = (-height).atan2(r);
                (
                    glam::Vec3::new(x, height, z),
                    angle + std::f32::consts::PI,
                    pitch,
                )
            }
            Self::LargeWorld => {
                // Long low sweep across the world: crosses hundreds of
                // meters so streaming + eviction actually cycle.
                let angle = t * TAU;
                let r = 250.0 + 150.0 * (t * TAU).sin();
                let height = 15.0 + 20.0 * (t * TAU * 2.0).cos().abs();
                let x = r * angle.cos();
                let z = r * angle.sin();
                let pitch = (-height * 0.3).atan2(60.0);
                (
                    glam::Vec3::new(x, height, z),
                    angle + std::f32::consts::PI,
                    pitch,
                )
            }
            Self::TerrainOnly => {
                let angle = t * TAU;
                let r = 20.0;
                let height = 4.0 + 3.0 * (t * TAU * 3.0).sin();
                let x = r * angle.cos();
                let z = r * angle.sin();
                let pitch = (-height).atan2(r);
                (
                    glam::Vec3::new(x, height, z),
                    angle + std::f32::consts::PI,
                    pitch,
                )
            }
            Self::ObjectsOnly => {
                let angle = t * TAU;
                let r = 8.0 + 4.0 * (t * TAU * 2.0).sin();
                let height = 2.5 + 1.5 * (t * TAU * 3.0).cos();
                let x = r * angle.cos();
                let z = r * angle.sin();
                let pitch = (-height).atan2(r);
                (
                    glam::Vec3::new(x, height, z),
                    angle + std::f32::consts::PI,
                    pitch,
                )
            }
            Self::Stress => {
                let angle = t * TAU;
                let r = 30.0 - 15.0 * (t * TAU).sin().abs();
                let height = 10.0 + 10.0 * (t * TAU * 2.0).cos();
                let x = r * angle.cos();
                let z = r * angle.sin();
                let pitch = (-height).atan2(r);
                (
                    glam::Vec3::new(x, height, z),
                    angle + std::f32::consts::PI,
                    pitch,
                )
            }
            Self::SingleBrick => {
                let angle = t * TAU;
                let r = 2.5;
                let height = 1.0 + 0.5 * (t * TAU * 2.0).sin();
                let x = 0.8 + r * angle.cos();
                let z = 0.8 + r * angle.sin();
                let pitch = (-(height - 0.8)).atan2(r);
                (
                    glam::Vec3::new(x, height, z),
                    angle + std::f32::consts::PI,
                    pitch,
                )
            }
            Self::Empty => (glam::Vec3::new(0.0, 2.0, 5.0), 0.0, 0.0),
        }
    }

    /// World radius preloaded synchronously at startup for streaming presets.
    fn preload_radius(self) -> f32 {
        match self {
            Self::LargeWorld => 60.0,
            _ => f32::INFINITY,
        }
    }

    /// Sets up the world. Returns a `BrickPager` for presets that stream terrain.
    pub fn setup(
        self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pool: &mut BrickPool,
        svo: &mut Svo,
        world: &mut World,
    ) -> Option<BrickPager> {
        match self {
            Self::Default => {
                let mut pager = create_terrain_pager(self, device, queue, pool.capacity(), svo);
                pager.preload_all(svo, pool, queue);
                populate_default_objects(queue, pool, world, svo);
                Some(pager)
            }
            Self::TerrainOnly => {
                let mut pager = create_terrain_pager(self, device, queue, pool.capacity(), svo);
                pager.preload_all(svo, pool, queue);
                Some(pager)
            }
            Self::LargeWorld => {
                let mut pager = create_terrain_pager(self, device, queue, pool.capacity(), svo);
                let (cam, _, _) = self.camera_start();
                pager.preload_radius(cam, self.preload_radius(), svo, pool, queue);
                Some(pager)
            }
            Self::ObjectsOnly => {
                populate_objects_only(queue, pool, world);
                None
            }
            Self::Stress => {
                populate_stress(queue, pool, world);
                None
            }
            Self::SingleBrick => {
                setup_single_brick(queue, pool, svo);
                None
            }
            Self::Empty => None,
        }
    }
}

/// Builds the coarse SVO for the whole world, primes the GPU worldgen cache
/// for the cells that will be preloaded, and constructs the streaming pager
/// over the coarse pass's candidate set.
fn create_terrain_pager(
    preset: Preset,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pool_capacity: u32,
    svo: &mut Svo,
) -> BrickPager {
    let dims = preset.grid_dims();
    let world_min = preset.world_min();
    let generator = WorldGenerator::new(42);

    let coarse = coarse_svo::build_coarse(svo, &generator, dims);

    // Prime the GPU worldgen cache with exactly the cells the preload will
    // request: candidates within the preload radius of the camera start.
    let brick_size = BRICK_EDGE as f32 * VOXEL_SCALE;
    let (cam, _, _) = preset.camera_start();
    let radius = preset.preload_radius();
    let r2 = radius * radius;
    let mut prime: Vec<[u32; 3]> = Vec::new();
    for gz in 0..dims[2] {
        for gx in 0..dims[0] {
            let (lo, hi) = coarse.column_range[(gx + dims[0] * gz) as usize];
            if lo > hi {
                continue;
            }
            for gy in u32::from(lo)..=u32::from(hi) {
                let center = world_min
                    + glam::Vec3::new(gx as f32 + 0.5, gy as f32 + 0.5, gz as f32 + 0.5)
                        * brick_size;
                if radius.is_infinite() || center.distance_squared(cam) <= r2 {
                    prime.push([gx, gy, gz]);
                }
            }
        }
    }
    let mut gpu_gen = GpuWorldGenerator::new(device, 42, world_min);
    gpu_gen.generate_cells(&prime, device, queue);

    let cache_label = preset.label().to_lowercase().replace(' ', "_");
    let cache_dir = cached_source::cache_dir_for_preset(&cache_label);
    let source = GpuCachedSource::new(gpu_gen.cache(), WorldGenerator::new(42), cache_dir);

    let config = PagerConfig {
        worker_threads: if preset == Preset::LargeWorld { 8 } else { 4 },
        ..PagerConfig::default()
    };

    BrickPager::new(
        Arc::new(source),
        dims,
        world_min,
        coarse.column_range,
        pool_capacity,
        config,
    )
}

fn populate_default_objects(
    queue: &wgpu::Queue,
    pool: &mut BrickPool,
    world: &mut World,
    terrain: &Svo,
) {
    let start = Instant::now();

    let tree_model = model_gen::generate_tree(pool, queue, 42);
    let rock_model = model_gen::generate_rock(pool, queue, 99);
    let pebble_model = model_gen::generate_pebble(pool, queue, 77);
    let tree_id = world.add_model(tree_model);
    let rock_id = world.add_model(rock_model);
    let pebble_id = world.add_model(pebble_model);

    let wg = WorldGenerator::new(42);
    let spacing = 4.0_f32;
    let world_min = terrain.world_min();
    let world_max = world_min + glam::Vec3::splat(terrain.world_size());

    let tree_ext = world.models()[tree_id].world_extent();
    let rock_ext = world.models()[rock_id].world_extent();
    let pebble_ext = world.models()[pebble_id].world_extent();

    let mut tree_count = 0u32;
    let mut rock_count = 0u32;
    let mut pebble_count = 0u32;

    let mut x = world_min.x + spacing;
    while x < world_max.x - spacing {
        let mut z = world_min.z + spacing;
        while z < world_max.z - spacing {
            let h = worldgen::hash_for_placement(x, z, 42);
            if let Some(surface_y) = wg.find_surface_y(x, z)
                && surface_y > 0.0
                && surface_y < world_max.y - 3.0
            {
                if h.is_multiple_of(5) && tree_count < 100 {
                    world.add_instance(VoxelInstance {
                        model_id: tree_id,
                        position: glam::Vec3::new(x, surface_y + tree_ext.y * 0.5, z),
                        rotation: glam::Quat::from_rotation_y((h % 628) as f32 / 100.0),
                    });
                    tree_count += 1;
                } else if h.is_multiple_of(11) && rock_count < 50 {
                    world.add_instance(VoxelInstance {
                        model_id: rock_id,
                        position: glam::Vec3::new(x, surface_y + rock_ext.y * 0.5, z),
                        rotation: glam::Quat::from_rotation_y((h % 314) as f32 / 100.0),
                    });
                    rock_count += 1;
                }
            }
            z += spacing;
        }
        x += spacing;
    }

    let pebble_spacing = 1.5_f32;
    let mut px = world_min.x + 1.0;
    while px < world_max.x - 1.0 {
        let mut pz = world_min.z + 1.0;
        while pz < world_max.z - 1.0 {
            let h = worldgen::hash_for_placement(px, pz, 9999);
            if h.is_multiple_of(3)
                && let Some(sy) = wg.find_surface_y(px, pz)
                && sy > 0.0
                && sy < world_max.y - 1.0
                && pebble_count < 500
            {
                world.add_instance(VoxelInstance {
                    model_id: pebble_id,
                    position: glam::Vec3::new(px, sy + pebble_ext.y * 0.5, pz),
                    rotation: glam::Quat::from_rotation_y((h % 628) as f32 / 100.0),
                });
                pebble_count += 1;
            }
            pz += pebble_spacing;
        }
        px += pebble_spacing;
    }

    let elapsed = start.elapsed();
    log::info!(
        "scene: {tree_count} trees + {rock_count} rocks + {pebble_count} pebbles in {:.1} ms",
        elapsed.as_secs_f64() * 1000.0
    );
}

fn populate_objects_only(queue: &wgpu::Queue, pool: &mut BrickPool, world: &mut World) {
    let start = Instant::now();

    let tree_model = model_gen::generate_tree(pool, queue, 42);
    let rock_model = model_gen::generate_rock(pool, queue, 99);
    let tree_id = world.add_model(tree_model);
    let rock_id = world.add_model(rock_model);

    let tree_ext = world.models()[tree_id].world_extent();
    let rock_ext = world.models()[rock_id].world_extent();

    let mut tree_count = 0u32;
    let mut rock_count = 0u32;
    let spacing = 6.0_f32;

    let mut x = -20.0_f32;
    while x < 20.0 {
        let mut z = -20.0_f32;
        while z < 20.0 {
            let h = worldgen::hash_for_placement(x, z, 42);
            if h.is_multiple_of(5) && tree_count < 30 {
                world.add_instance(VoxelInstance {
                    model_id: tree_id,
                    position: glam::Vec3::new(x, tree_ext.y * 0.5, z),
                    rotation: glam::Quat::from_rotation_y((h % 628) as f32 / 100.0),
                });
                tree_count += 1;
            } else if h.is_multiple_of(7) && rock_count < 50 {
                world.add_instance(VoxelInstance {
                    model_id: rock_id,
                    position: glam::Vec3::new(x, rock_ext.y * 0.5, z),
                    rotation: glam::Quat::from_rotation_y((h % 314) as f32 / 100.0),
                });
                rock_count += 1;
            }
            z += spacing;
        }
        x += spacing;
    }

    let elapsed = start.elapsed();
    log::info!(
        "objects-only: {tree_count} trees + {rock_count} rocks in {:.1} ms",
        elapsed.as_secs_f64() * 1000.0
    );
}

fn populate_stress(queue: &wgpu::Queue, pool: &mut BrickPool, world: &mut World) {
    let start = Instant::now();

    let tree_model = model_gen::generate_tree(pool, queue, 42);
    let rock_model = model_gen::generate_rock(pool, queue, 99);
    let pebble_model = model_gen::generate_pebble(pool, queue, 77);
    let tree_id = world.add_model(tree_model);
    let rock_id = world.add_model(rock_model);
    let pebble_id = world.add_model(pebble_model);

    let tree_ext = world.models()[tree_id].world_extent();
    let rock_ext = world.models()[rock_id].world_extent();
    let pebble_ext = world.models()[pebble_id].world_extent();

    let mut count = 0u32;
    let spacing = 2.0_f32;
    let half = 22.0_f32;

    let mut x = -half;
    while x < half {
        let mut z = -half;
        while z < half {
            let h = worldgen::hash_for_placement(x, z, 7777);
            let (model_id, ext_y) = match h % 3 {
                0 => (tree_id, tree_ext.y),
                1 => (rock_id, rock_ext.y),
                _ => (pebble_id, pebble_ext.y),
            };
            world.add_instance(VoxelInstance {
                model_id,
                position: glam::Vec3::new(x, ext_y * 0.5, z),
                rotation: glam::Quat::from_rotation_y((h % 628) as f32 / 100.0),
            });
            count += 1;
            z += spacing;
        }
        x += spacing;
    }

    let elapsed = start.elapsed();
    log::info!(
        "stress: {count} instances in {:.1} ms",
        elapsed.as_secs_f64() * 1000.0
    );
}

fn setup_single_brick(queue: &wgpu::Queue, pool: &mut BrickPool, svo: &mut Svo) {
    let handle = pool.alloc().expect("brick pool exhausted");

    let mut voxels = [0u8; BRICK_VOLUME as usize];
    for lz in 0..BRICK_EDGE {
        for ly in 0..BRICK_EDGE {
            for lx in 0..BRICK_EDGE {
                let idx = (lx + BRICK_EDGE * (ly + BRICK_EDGE * lz)) as usize;
                let face_x = lx == 0 || lx == BRICK_EDGE - 1;
                let face_y = ly == 0 || ly == BRICK_EDGE - 1;
                let face_z = lz == 0 || lz == BRICK_EDGE - 1;
                let face_count = face_x as u8 + face_y as u8 + face_z as u8;
                voxels[idx] = match face_count {
                    0 => 1,
                    1 => 2,
                    2 => 3,
                    _ => 4,
                };
            }
        }
    }

    let palette: &[[u8; 4]] = &[
        [0, 0, 0, 0],
        [180, 40, 40, 255],
        [40, 180, 40, 255],
        [40, 40, 180, 255],
        [200, 200, 40, 255],
    ];

    pool.write_voxels(queue, handle, &voxels);
    pool.write_palette(queue, handle, palette);

    let brick_size = BRICK_EDGE as f32 * VOXEL_SCALE;
    let world_pos = svo.world_min() + glam::Vec3::new(1.0, 1.0, 1.0) * brick_size;
    svo.insert_brick(world_pos, brick_size, handle, [128, 128, 128, 255]);
    svo.update_colors();
    svo.upload(queue);

    log::info!("single-brick: 1 brick with debug material palette");
}
