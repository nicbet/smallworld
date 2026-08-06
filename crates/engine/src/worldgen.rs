//! Procedural world generator: 3D density terrain with strata, caves, and water.

use crate::brick_pool::{BRICK_EDGE, BRICK_VOLUME, VOXEL_SCALE};

const MAT_AIR: u8 = 0;
const MAT_GRASS: u8 = 1;
const MAT_DIRT: u8 = 2;
const MAT_STONE: u8 = 3;
const MAT_DARK_STONE: u8 = 4;
const MAT_WATER: u8 = 5;
const MAT_WOOD: u8 = 6;
const MAT_LEAVES: u8 = 7;
const MAT_GRASS_ALT: u8 = 8;

/// Shared palette for all generated bricks.
pub const PALETTE: &[[u8; 4]] = &[
    [0, 0, 0, 0],         // 0 air
    [76, 153, 0, 255],    // 1 grass
    [139, 90, 43, 255],   // 2 dirt
    [128, 128, 128, 255], // 3 stone
    [80, 80, 90, 255],    // 4 dark stone
    [30, 100, 180, 255],  // 5 water
    [90, 60, 30, 255],    // 6 wood
    [40, 120, 20, 255],   // 7 leaves
    [55, 130, 15, 255],   // 8 tall grass
];

/// Data for one generated brick.
pub struct BrickData {
    /// 16³ voxel material indices.
    pub voxels: [u8; BRICK_VOLUME as usize],
    /// Palette entries used by this brick.
    pub palette: &'static [[u8; 4]],
}

/// Deterministic 3D density-based terrain generator.
pub struct WorldGenerator {
    seed: u32,
    terrain_base: f32,
    terrain_amp: f32,
    cave_threshold: f32,
    water_level: f32,
}

impl WorldGenerator {
    /// Creates a generator with the given seed.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            terrain_base: 2.0,
            terrain_amp: 8.0,
            cave_threshold: 0.48,
            water_level: -1.0,
        }
    }

    /// Generates voxel data for one brick at the given grid position.
    /// Returns `None` if the brick is entirely empty (air above water table).
    #[must_use]
    pub fn generate_brick(
        &self,
        grid_pos: [u32; 3],
        world_min: glam::Vec3,
    ) -> Option<BrickData> {
        let brick_min = world_min
            + glam::Vec3::new(
                grid_pos[0] as f32,
                grid_pos[1] as f32,
                grid_pos[2] as f32,
            ) * (BRICK_EDGE as f32 * VOXEL_SCALE);

        let mut voxels = [MAT_AIR; BRICK_VOLUME as usize];
        let mut has_content = false;

        for lz in 0..BRICK_EDGE {
            for ly in 0..BRICK_EDGE {
                for lx in 0..BRICK_EDGE {
                    let wx = brick_min.x + (lx as f32 + 0.5) * VOXEL_SCALE;
                    let wy = brick_min.y + (ly as f32 + 0.5) * VOXEL_SCALE;
                    let wz = brick_min.z + (lz as f32 + 0.5) * VOXEL_SCALE;

                    let mat = self.sample(wx, wy, wz);
                    if mat != MAT_AIR {
                        has_content = true;
                    }
                    let idx = (lx + BRICK_EDGE * (ly + BRICK_EDGE * lz)) as usize;
                    voxels[idx] = mat;
                }
            }
        }

        if !has_content {
            return None;
        }

        Some(BrickData {
            voxels,
            palette: PALETTE,
        })
    }

    fn sample(&self, wx: f32, wy: f32, wz: f32) -> u8 {
        let terrain_noise = fbm3d(wx * 0.012, wy * 0.03, wz * 0.012, 3, self.seed) - 0.5;
        let density = (self.terrain_base - wy) / self.terrain_amp + terrain_noise;

        if density <= 0.0 {
            // Above terrain — check for trees and vegetation
            if wy > self.water_level {
                if let Some(mat) = self.check_tree(wx, wy, wz) {
                    return mat;
                }
                return MAT_AIR;
            }
            return MAT_WATER;
        }

        let cave_a = fbm3d(wx * 0.08, wy * 0.08, wz * 0.08, 2, self.seed.wrapping_add(777));
        let cave_b = fbm3d(wx * 0.08, wy * 0.1, wz * 0.08, 2, self.seed.wrapping_add(1234));
        if cave_a > self.cave_threshold && cave_b > self.cave_threshold && density < 1.5 {
            if wy <= self.water_level {
                return MAT_WATER;
            }
            return MAT_AIR;
        }

        if density < 0.08 {
            let h = hash((wx * 10.0) as i32, (wy * 10.0) as i32, (wz * 10.0) as i32, self.seed.wrapping_add(2222));
            if h.is_multiple_of(3) { MAT_GRASS_ALT } else { MAT_GRASS }
        } else if density < 0.25 {
            MAT_DIRT
        } else if wy > -5.0 {
            MAT_STONE
        } else {
            MAT_DARK_STONE
        }
    }

    /// Approximate terrain surface height at (wx, wz).
    #[must_use]
    pub fn approx_surface_y(&self, wx: f32, wz: f32) -> f32 {
        let noise = fbm3d(wx * 0.012, 0.0, wz * 0.012, 3, self.seed) - 0.5;
        self.terrain_base + noise * self.terrain_amp
    }

    fn check_tree(&self, wx: f32, wy: f32, wz: f32) -> Option<u8> {
        let spacing = 4.0_f32;
        let base_gx = (wx / spacing).floor() as i32;
        let base_gz = (wz / spacing).floor() as i32;

        for dz in -1..=1 {
            for dx in -1..=1 {
                let gx = base_gx + dx;
                let gz = base_gz + dz;

                let h = hash(gx, 0, gz, self.seed.wrapping_add(5555));
                if !h.is_multiple_of(7) {
                    continue;
                }

                let jx = hash_f(gx, 1, gz, self.seed.wrapping_add(6666));
                let jz = hash_f(gx, 2, gz, self.seed.wrapping_add(7777));
                let tx = (gx as f32 + 0.1 + jx * 0.8) * spacing;
                let tz = (gz as f32 + 0.1 + jz * 0.8) * spacing;

                let surface_y = self.approx_surface_y(tx, tz);
                if surface_y <= self.water_level {
                    continue;
                }

                let rel_x = wx - tx;
                let rel_z = wz - tz;
                let rel_y = wy - surface_y;

                let tree_h = 2.5 + hash_f(gx, 3, gz, self.seed.wrapping_add(8888)) * 3.0;
                let trunk_r = 0.12;

                if rel_x * rel_x + rel_z * rel_z < trunk_r * trunk_r
                    && rel_y >= 0.0
                    && rel_y < tree_h * 0.7
                {
                    return Some(MAT_WOOD);
                }

                let canopy_y = tree_h * 0.55;
                let canopy_r = 0.8 + hash_f(gx, 4, gz, self.seed.wrapping_add(9999)) * 1.2;
                let cy = rel_y - canopy_y;
                let canopy_r_y = canopy_r * 0.7;
                let d = (rel_x * rel_x + rel_z * rel_z) / (canopy_r * canopy_r)
                    + (cy * cy) / (canopy_r_y * canopy_r_y);
                if d < 1.0 && rel_y > 0.0 {
                    return Some(MAT_LEAVES);
                }
            }
        }
        None
    }

}

/// Deterministic hash for object placement at a world (x, z) coordinate.
#[must_use]
pub fn hash_for_placement(x: f32, z: f32, seed: u32) -> u32 {
    hash((x * 100.0) as i32, 0, (z * 100.0) as i32, seed.wrapping_add(55555))
}

// ---------------------------------------------------------------------------
// Self-contained noise (no external crate)
// ---------------------------------------------------------------------------

fn hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed;
    h = h.wrapping_add(x as u32).wrapping_mul(0x9e3779b9);
    h ^= h >> 16;
    h = h.wrapping_add(y as u32).wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_add(z as u32).wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h
}

fn hash_f(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    (hash(x, y, z, seed) & 0x7FFF) as f32 / 32767.0
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn noise3d(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let iz = z.floor() as i32;
    let fx = smoothstep(x - ix as f32);
    let fy = smoothstep(y - iy as f32);
    let fz = smoothstep(z - iz as f32);

    let c000 = hash_f(ix, iy, iz, seed);
    let c100 = hash_f(ix + 1, iy, iz, seed);
    let c010 = hash_f(ix, iy + 1, iz, seed);
    let c110 = hash_f(ix + 1, iy + 1, iz, seed);
    let c001 = hash_f(ix, iy, iz + 1, seed);
    let c101 = hash_f(ix + 1, iy, iz + 1, seed);
    let c011 = hash_f(ix, iy + 1, iz + 1, seed);
    let c111 = hash_f(ix + 1, iy + 1, iz + 1, seed);

    let x0 = lerp(c000, c100, fx);
    let x1 = lerp(c010, c110, fx);
    let x2 = lerp(c001, c101, fx);
    let x3 = lerp(c011, c111, fx);

    let y0 = lerp(x0, x1, fy);
    let y1 = lerp(x2, x3, fy);

    lerp(y0, y1, fz)
}

fn fbm3d(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    let mut value = 0.0_f32;
    let mut amp = 1.0_f32;
    let mut freq = 1.0_f32;
    let mut max_amp = 0.0_f32;
    for i in 0..octaves {
        value += noise3d(x * freq, y * freq, z * freq, seed.wrapping_add(i * 31)) * amp;
        max_amp += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    value / max_amp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic() {
        let a = fbm3d(1.5, 2.3, 0.7, 4, 42);
        let b = fbm3d(1.5, 2.3, 0.7, 4, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn noise_range_is_zero_to_one() {
        for i in 0..1000 {
            let v = fbm3d(i as f32 * 0.1, 0.0, 0.0, 5, 42);
            assert!((0.0..=1.0).contains(&v), "noise out of range: {v}");
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = fbm3d(1.0, 2.0, 3.0, 4, 1);
        let b = fbm3d(1.0, 2.0, 3.0, 4, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn generator_produces_mixed_bricks() {
        let wg = WorldGenerator::new(42);
        let world_min = glam::Vec3::new(-25.6, -12.8, -25.6);
        let mut solid = 0;
        let mut empty = 0;
        for gy in 0..16u32 {
            let result = wg.generate_brick([8, gy, 8], world_min);
            if result.is_some() { solid += 1; } else { empty += 1; }
        }
        assert!(solid > 0, "no solid bricks at all");
        assert!(empty > 0, "no empty bricks — terrain should have sky above");
    }
}
