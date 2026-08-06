//! Intra-brick mip chain: 4 levels (8³, 4³, 2³, 1³) of pre-averaged RGBA.

use crate::brick_pool::BRICK_EDGE;

/// u32 words per brick in the mip buffer (8³ + 4³ + 2³ + 1³).
pub const MIP_WORDS_PER_BRICK: u32 = 512 + 64 + 8 + 1;

fn pack_avg(r_sum: u32, g_sum: u32, b_sum: u32, a_val: u32, count: u32) -> u32 {
    let r = (r_sum / count) & 0xFF;
    let g = (g_sum / count) & 0xFF;
    let b = (b_sum / count) & 0xFF;
    let a = a_val & 0xFF;
    r | (g << 8) | (b << 16) | (a << 24)
}

/// Builds 4 mip levels from a brick's voxel data and palette.
pub fn compute_brick_mips(
    voxels: &[u8; 4096],
    palette: &[[u8; 4]],
) -> [u32; MIP_WORDS_PER_BRICK as usize] {
    let mut mips = [0u32; MIP_WORDS_PER_BRICK as usize];

    // Level 1 (8³): average 2×2×2 blocks of the 16³ source
    for mz in 0..8u32 {
        for my in 0..8u32 {
            for mx in 0..8u32 {
                let mut r_sum = 0u32;
                let mut g_sum = 0u32;
                let mut b_sum = 0u32;
                let mut solid = 0u32;

                for dz in 0..2u32 {
                    for dy in 0..2u32 {
                        for dx in 0..2u32 {
                            let sx = mx * 2 + dx;
                            let sy = my * 2 + dy;
                            let sz = mz * 2 + dz;
                            let idx = (sx + BRICK_EDGE * (sy + BRICK_EDGE * sz)) as usize;
                            let mat = voxels[idx];
                            if mat != 0 {
                                let pi = mat as usize;
                                let c = if pi < palette.len() {
                                    palette[pi]
                                } else {
                                    [128, 128, 128, 255]
                                };
                                r_sum += c[0] as u32;
                                g_sum += c[1] as u32;
                                b_sum += c[2] as u32;
                                solid += 1;
                            }
                        }
                    }
                }

                if solid > 0 {
                    let flat = (mx + 8 * (my + 8 * mz)) as usize;
                    mips[flat] = pack_avg(r_sum, g_sum, b_sum, solid * 255 / 8, solid);
                }
            }
        }
    }

    // Levels 2–4: filter from previous level
    let level_params: [(u32, u32, u32); 3] = [
        (8, 4, 512), // level 2: 4³, reads from level 1 (edge 8), writes at offset 512
        (4, 2, 576), // level 3: 2³, reads from level 2 (edge 4), writes at offset 576
        (2, 1, 584), // level 4: 1³, reads from level 3 (edge 2), writes at offset 584
    ];

    let mut prev_offset = 0u32;
    for &(src_edge, dst_edge, dst_offset) in &level_params {
        for mz in 0..dst_edge {
            for my in 0..dst_edge {
                for mx in 0..dst_edge {
                    let mut r_sum = 0u32;
                    let mut g_sum = 0u32;
                    let mut b_sum = 0u32;
                    let mut a_sum = 0u32;
                    let mut solid = 0u32;

                    for dz in 0..2u32 {
                        for dy in 0..2u32 {
                            for dx in 0..2u32 {
                                let sx = mx * 2 + dx;
                                let sy = my * 2 + dy;
                                let sz = mz * 2 + dz;
                                let flat = (sx + src_edge * (sy + src_edge * sz)) as usize;
                                let packed = mips[prev_offset as usize + flat];
                                let a = packed >> 24;
                                a_sum += a;
                                if a > 0 {
                                    r_sum += packed & 0xFF;
                                    g_sum += (packed >> 8) & 0xFF;
                                    b_sum += (packed >> 16) & 0xFF;
                                    solid += 1;
                                }
                            }
                        }
                    }

                    if solid > 0 {
                        let flat = (mx + dst_edge * (my + dst_edge * mz)) as usize;
                        mips[dst_offset as usize + flat] =
                            pack_avg(r_sum, g_sum, b_sum, a_sum / 8, solid);
                    }
                }
            }
        }
        prev_offset = dst_offset;
    }

    mips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_air_produces_zero_mips() {
        let voxels = [0u8; 4096];
        let palette: &[[u8; 4]] = &[[0, 0, 0, 0]];
        let mips = compute_brick_mips(&voxels, palette);
        assert!(mips.iter().all(|&v| v == 0));
    }

    #[test]
    fn fully_solid_produces_nonzero_at_all_levels() {
        let voxels = [1u8; 4096];
        let palette: &[[u8; 4]] = &[[0, 0, 0, 0], [200, 100, 50, 255]];
        let mips = compute_brick_mips(&voxels, palette);

        // Level 1: 512 entries, all should be nonzero
        assert!(mips[..512].iter().all(|&v| v != 0));
        // Level 4 (top): single entry at offset 584
        let top = mips[584];
        assert_ne!(top, 0);
        let r = top & 0xFF;
        let g = (top >> 8) & 0xFF;
        let b = (top >> 16) & 0xFF;
        assert_eq!(r, 200);
        assert_eq!(g, 100);
        assert_eq!(b, 50);
    }

    #[test]
    fn half_solid_has_correct_occupancy() {
        let mut voxels = [0u8; 4096];
        // Fill bottom half (y < 8)
        for z in 0..16u32 {
            for y in 0..8u32 {
                for x in 0..16u32 {
                    voxels[(x + 16 * (y + 16 * z)) as usize] = 1;
                }
            }
        }
        let palette: &[[u8; 4]] = &[[0, 0, 0, 0], [100, 100, 100, 255]];
        let mips = compute_brick_mips(&voxels, palette);

        // Top mip: occupancy should be ~50% = 127 or 128
        let top = mips[584];
        let a = top >> 24;
        assert!(a > 100 && a < 160, "expected ~50% occupancy, got {a}");
    }
}
