use std::time::Instant;

use smallworld_engine::brick_index::BrickIndex;
use smallworld_engine::brick_pool::{BRICK_EDGE, BRICK_VOLUME, BrickPool, VOXEL_SCALE};
use smallworld_engine::scene::Scene;
use smallworld_engine::voxel_object::VoxelInstance;
use smallworld_engine::wgpu;

use crate::model_gen;
use crate::worldgen::{self, WorldGenerator};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Default,
    TerrainOnly,
    ObjectsOnly,
    Stress,
    SingleBrick,
    Empty,
}

impl Preset {
    pub const ALL: &[Self] = &[
        Self::Default,
        Self::TerrainOnly,
        Self::ObjectsOnly,
        Self::Stress,
        Self::SingleBrick,
        Self::Empty,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::TerrainOnly => "Terrain Only",
            Self::ObjectsOnly => "Objects Only",
            Self::Stress => "Stress",
            Self::SingleBrick => "Single Brick",
            Self::Empty => "Empty",
        }
    }

    pub fn grid_dims(self) -> [u32; 3] {
        match self {
            Self::Default | Self::TerrainOnly => [32, 12, 32],
            Self::ObjectsOnly | Self::Stress => [4, 2, 4],
            Self::SingleBrick | Self::Empty => [2, 2, 2],
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
            Self::Default | Self::TerrainOnly => 32768,
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

    pub fn setup(
        self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        pool: &mut BrickPool,
        index: &mut BrickIndex,
        scene: &mut Scene,
    ) {
        match self {
            Self::Default => {
                generate_terrain(queue, pool, index);
                populate_default_objects(queue, pool, scene, index);
            }
            Self::TerrainOnly => {
                generate_terrain(queue, pool, index);
            }
            Self::ObjectsOnly => {
                populate_objects_only(queue, pool, scene);
            }
            Self::Stress => {
                populate_stress(queue, pool, scene);
            }
            Self::SingleBrick => {
                setup_single_brick(queue, pool, index);
            }
            Self::Empty => {}
        }
    }
}

fn generate_terrain(queue: &wgpu::Queue, pool: &mut BrickPool, index: &mut BrickIndex) {
    let start = Instant::now();
    let wg = WorldGenerator::new(42);
    let dims = index.dims();
    let world_min = index.world_min();

    let mut allocated = 0u32;
    for gz in 0..dims[2] {
        for gy in 0..dims[1] {
            for gx in 0..dims[0] {
                if let Some(data) = wg.generate_brick([gx, gy, gz], world_min) {
                    let handle = pool.alloc().expect("brick pool exhausted");
                    pool.write_voxels(queue, handle, &data.voxels);
                    pool.write_palette(queue, handle, data.palette);
                    let mips =
                        smallworld_engine::mip::compute_brick_mips(&data.voxels, data.palette);
                    pool.write_mips(queue, handle, &mips);
                    index.set([gx, gy, gz], handle);
                    allocated += 1;
                }
            }
        }
    }
    index.upload(queue);
    let elapsed = start.elapsed();
    log::info!(
        "worldgen: {allocated} bricks in {:.1} ms (seed 42)",
        elapsed.as_secs_f64() * 1000.0
    );
}

fn populate_default_objects(
    queue: &wgpu::Queue,
    pool: &mut BrickPool,
    scene: &mut Scene,
    terrain: &BrickIndex,
) {
    let start = Instant::now();

    let tree_model = model_gen::generate_tree(pool, queue, 42);
    let rock_model = model_gen::generate_rock(pool, queue, 99);
    let pebble_model = model_gen::generate_pebble(pool, queue, 77);
    let tree_id = scene.add_model(tree_model);
    let rock_id = scene.add_model(rock_model);
    let pebble_id = scene.add_model(pebble_model);

    let wg = WorldGenerator::new(42);
    let spacing = 4.0_f32;
    let world_min = terrain.world_min();
    let dims = terrain.dims();
    let world_max = world_min
        + glam::Vec3::new(dims[0] as f32, dims[1] as f32, dims[2] as f32) * terrain.brick_size();

    let tree_ext = scene.models()[tree_id].world_extent();
    let rock_ext = scene.models()[rock_id].world_extent();
    let pebble_ext = scene.models()[pebble_id].world_extent();

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
                    scene.add_instance(VoxelInstance {
                        model_id: tree_id,
                        position: glam::Vec3::new(x, surface_y + tree_ext.y * 0.5, z),
                        rotation: glam::Quat::from_rotation_y((h % 628) as f32 / 100.0),
                    });
                    tree_count += 1;
                } else if h.is_multiple_of(11) && rock_count < 50 {
                    scene.add_instance(VoxelInstance {
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
                scene.add_instance(VoxelInstance {
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

fn populate_objects_only(queue: &wgpu::Queue, pool: &mut BrickPool, scene: &mut Scene) {
    let start = Instant::now();

    let tree_model = model_gen::generate_tree(pool, queue, 42);
    let rock_model = model_gen::generate_rock(pool, queue, 99);
    let tree_id = scene.add_model(tree_model);
    let rock_id = scene.add_model(rock_model);

    let tree_ext = scene.models()[tree_id].world_extent();
    let rock_ext = scene.models()[rock_id].world_extent();

    let mut tree_count = 0u32;
    let mut rock_count = 0u32;
    let spacing = 6.0_f32;

    let mut x = -20.0_f32;
    while x < 20.0 {
        let mut z = -20.0_f32;
        while z < 20.0 {
            let h = worldgen::hash_for_placement(x, z, 42);
            if h.is_multiple_of(5) && tree_count < 30 {
                scene.add_instance(VoxelInstance {
                    model_id: tree_id,
                    position: glam::Vec3::new(x, tree_ext.y * 0.5, z),
                    rotation: glam::Quat::from_rotation_y((h % 628) as f32 / 100.0),
                });
                tree_count += 1;
            } else if h.is_multiple_of(7) && rock_count < 50 {
                scene.add_instance(VoxelInstance {
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

fn populate_stress(queue: &wgpu::Queue, pool: &mut BrickPool, scene: &mut Scene) {
    let start = Instant::now();

    let tree_model = model_gen::generate_tree(pool, queue, 42);
    let rock_model = model_gen::generate_rock(pool, queue, 99);
    let pebble_model = model_gen::generate_pebble(pool, queue, 77);
    let tree_id = scene.add_model(tree_model);
    let rock_id = scene.add_model(rock_model);
    let pebble_id = scene.add_model(pebble_model);

    let tree_ext = scene.models()[tree_id].world_extent();
    let rock_ext = scene.models()[rock_id].world_extent();
    let pebble_ext = scene.models()[pebble_id].world_extent();

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
            scene.add_instance(VoxelInstance {
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

fn setup_single_brick(queue: &wgpu::Queue, pool: &mut BrickPool, index: &mut BrickIndex) {
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
    let mips = smallworld_engine::mip::compute_brick_mips(&voxels, palette);
    pool.write_mips(queue, handle, &mips);
    index.set([1, 1, 1], handle);
    index.upload(queue);

    log::info!("single-brick: 1 brick with debug material palette");
}
