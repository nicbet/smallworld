//! Procedural voxel model generators (trees, rocks).

use crate::brick_pool::{BrickPool, BRICK_EDGE, BRICK_VOLUME};
use crate::voxel_object::VoxelModel;

const FINE_SCALE: f32 = 0.025;

const PALETTE_TREE: &[[u8; 4]] = &[
    [0, 0, 0, 0],         // 0 air
    [90, 60, 30, 255],    // 1 bark
    [35, 110, 20, 255],   // 2 leaves dark
    [50, 130, 25, 255],   // 3 leaves mid
    [65, 145, 35, 255],   // 4 leaves light
];

const PALETTE_ROCK: &[[u8; 4]] = &[
    [0, 0, 0, 0],         // 0 air
    [120, 115, 110, 255], // 1 rock light
    [95, 90, 85, 255],    // 2 rock dark
    [80, 80, 75, 255],    // 3 rock darkest
];

/// Generates a tree model at 2.5cm voxel scale.
pub fn generate_tree(pool: &mut BrickPool, queue: &wgpu::Queue, seed: u32) -> VoxelModel {
    let trunk_height_m = 2.0 + pseudo_f(seed, 100) * 2.0;
    let canopy_radius_m = 0.8 + pseudo_f(seed, 101) * 0.8;
    let canopy_height_m = canopy_radius_m * 1.4;
    let trunk_radius_m = 0.06 + pseudo_f(seed, 102) * 0.04;
    let total_height_m = trunk_height_m + canopy_height_m;

    let brick_size = BRICK_EDGE as f32 * FINE_SCALE;
    let gx = ((canopy_radius_m * 2.0) / brick_size).ceil() as u32 + 2;
    let gy = (total_height_m / brick_size).ceil() as u32 + 2;
    let gz = gx;
    let dims = [gx, gy, gz];

    let mut model = VoxelModel::new(dims, FINE_SCALE);

    let center_x = (gx as f32 * brick_size) / 2.0;
    let center_z = (gz as f32 * brick_size) / 2.0;
    let canopy_center_y = trunk_height_m * 0.65;

    for bgz in 0..gz {
        for bgy in 0..gy {
            for bgx in 0..gx {
                let mut voxels = [0u8; BRICK_VOLUME as usize];
                let mut has_solid = false;
                let brick_min_x = bgx as f32 * brick_size;
                let brick_min_y = bgy as f32 * brick_size;
                let brick_min_z = bgz as f32 * brick_size;

                for lz in 0..BRICK_EDGE {
                    for ly in 0..BRICK_EDGE {
                        for lx in 0..BRICK_EDGE {
                            let x = brick_min_x + (lx as f32 + 0.5) * FINE_SCALE - center_x;
                            let y = brick_min_y + (ly as f32 + 0.5) * FINE_SCALE;
                            let z = brick_min_z + (lz as f32 + 0.5) * FINE_SCALE - center_z;
                            let idx = (lx + BRICK_EDGE * (ly + BRICK_EDGE * lz)) as usize;

                            let horiz = (x * x + z * z).sqrt();

                            // Trunk
                            if horiz < trunk_radius_m && y < trunk_height_m * 0.75 && y >= 0.0 {
                                voxels[idx] = 1;
                                has_solid = true;
                                continue;
                            }

                            // Canopy (ellipsoid)
                            let cy = y - canopy_center_y;
                            let d = (x * x + z * z) / (canopy_radius_m * canopy_radius_m)
                                + (cy * cy) / (canopy_height_m * canopy_height_m);
                            if d < 1.0 && y > trunk_height_m * 0.3 {
                                let h = pseudo_u((x * 50.0) as i32, (y * 50.0) as i32, (z * 50.0) as i32, seed);
                                voxels[idx] = 2 + (h % 3) as u8;
                                has_solid = true;
                            }
                        }
                    }
                }

                if has_solid {
                    model.fill_brick([bgx, bgy, bgz], pool, queue, &voxels, PALETTE_TREE);
                }
            }
        }
    }

    model
}

/// Generates a rock model at 2.5cm voxel scale.
pub fn generate_rock(pool: &mut BrickPool, queue: &wgpu::Queue, seed: u32) -> VoxelModel {
    let radius_m = 0.2 + pseudo_f(seed, 200) * 0.3;
    let height_m = radius_m * (0.5 + pseudo_f(seed, 201) * 0.5);

    let brick_size = BRICK_EDGE as f32 * FINE_SCALE;
    let g = ((radius_m * 2.0) / brick_size).ceil() as u32 + 2;
    let gy = (height_m * 2.0 / brick_size).ceil() as u32 + 2;
    let dims = [g, gy, g];

    let mut model = VoxelModel::new(dims, FINE_SCALE);

    let center_x = (g as f32 * brick_size) / 2.0;
    let center_y = (gy as f32 * brick_size) / 2.0;
    let center_z = center_x;

    for bgz in 0..g {
        for bgy in 0..gy {
            for bgx in 0..g {
                let mut voxels = [0u8; BRICK_VOLUME as usize];
                let mut has_solid = false;
                let brick_min_x = bgx as f32 * brick_size;
                let brick_min_y = bgy as f32 * brick_size;
                let brick_min_z = bgz as f32 * brick_size;

                for lz in 0..BRICK_EDGE {
                    for ly in 0..BRICK_EDGE {
                        for lx in 0..BRICK_EDGE {
                            let x = brick_min_x + (lx as f32 + 0.5) * FINE_SCALE - center_x;
                            let y = brick_min_y + (ly as f32 + 0.5) * FINE_SCALE - center_y;
                            let z = brick_min_z + (lz as f32 + 0.5) * FINE_SCALE - center_z;
                            let idx = (lx + BRICK_EDGE * (ly + BRICK_EDGE * lz)) as usize;

                            let d = (x * x) / (radius_m * radius_m)
                                + (y * y) / (height_m * height_m)
                                + (z * z) / (radius_m * radius_m);

                            if d < 1.0 {
                                let h = pseudo_u((x * 40.0) as i32, (y * 40.0) as i32, (z * 40.0) as i32, seed);
                                voxels[idx] = 1 + (h % 3) as u8;
                                has_solid = true;
                            }
                        }
                    }
                }

                if has_solid {
                    model.fill_brick([bgx, bgy, bgz], pool, queue, &voxels, PALETTE_ROCK);
                }
            }
        }
    }

    model
}

/// Generates a small pebble at 2.5cm voxel scale (5-10cm across).
pub fn generate_pebble(pool: &mut BrickPool, queue: &wgpu::Queue, seed: u32) -> VoxelModel {
    let rx = 0.04 + pseudo_f(seed, 300) * 0.04;
    let ry = rx * (0.3 + pseudo_f(seed, 301) * 0.4);
    let rz = rx * (0.7 + pseudo_f(seed, 302) * 0.3);

    let brick_size = BRICK_EDGE as f32 * FINE_SCALE;
    let dims = [2u32, 2, 2];
    let mut model = VoxelModel::new(dims, FINE_SCALE);

    let cx = dims[0] as f32 * brick_size * 0.5;
    let cy = dims[1] as f32 * brick_size * 0.5;
    let cz = dims[2] as f32 * brick_size * 0.5;

    for bgz in 0..dims[2] {
        for bgy in 0..dims[1] {
            for bgx in 0..dims[0] {
                let mut voxels = [0u8; BRICK_VOLUME as usize];
                let mut has_solid = false;

                for lz in 0..BRICK_EDGE {
                    for ly in 0..BRICK_EDGE {
                        for lx in 0..BRICK_EDGE {
                            let x = bgx as f32 * brick_size + (lx as f32 + 0.5) * FINE_SCALE - cx;
                            let y = bgy as f32 * brick_size + (ly as f32 + 0.5) * FINE_SCALE - cy;
                            let z = bgz as f32 * brick_size + (lz as f32 + 0.5) * FINE_SCALE - cz;
                            let idx = (lx + BRICK_EDGE * (ly + BRICK_EDGE * lz)) as usize;

                            let d = (x * x) / (rx * rx) + (y * y) / (ry * ry) + (z * z) / (rz * rz);
                            if d < 1.0 {
                                let h = pseudo_u((x * 80.0) as i32, (y * 80.0) as i32, (z * 80.0) as i32, seed);
                                voxels[idx] = 1 + (h % 3) as u8;
                                has_solid = true;
                            }
                        }
                    }
                }

                if has_solid {
                    model.fill_brick([bgx, bgy, bgz], pool, queue, &voxels, PALETTE_ROCK);
                }
            }
        }
    }

    model
}

fn pseudo_u(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed;
    h = h.wrapping_add(x as u32).wrapping_mul(0x9e3779b9);
    h ^= h >> 16;
    h = h.wrapping_add(y as u32).wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_add(z as u32).wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h
}

fn pseudo_f(seed: u32, salt: u32) -> f32 {
    (pseudo_u(seed as i32, salt as i32, 0, 0) & 0xFFFF) as f32 / 65535.0
}
