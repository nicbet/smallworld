// Direct coarse mip generation — evaluates noise at sub-block centers.
// Generates levels 2-4 (73 entries) per brick without full 16³ voxel gen.

struct GenParams {
    seed: u32,
    terrain_base: f32,
    terrain_amp: f32,
    cave_threshold: f32,
    water_level: f32,
    brick_size: f32,
    _pad0: u32,
    _pad1: u32,
    world_min: vec3<f32>,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> params: GenParams;
@group(0) @binding(1) var<storage, read> requests: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;

const COARSE_MIP_WORDS: u32 = 73u;

// --- noise (same as worldgen.wgsl) ---

fn wg_hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    var h = seed;
    h = (h + u32(x)) * 0x9e3779b9u;
    h ^= h >> 16u;
    h = (h + u32(y)) * 0x85ebca6bu;
    h ^= h >> 13u;
    h = (h + u32(z)) * 0xc2b2ae35u;
    h ^= h >> 16u;
    return h;
}

fn hash_f(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    return f32(wg_hash(x, y, z, seed) & 0x7FFFu) / 32767.0;
}

fn smoothstep3(t: f32) -> f32 {
    return t * t * (3.0 - 2.0 * t);
}

fn noise3d(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let ix = i32(floor(x));
    let iy = i32(floor(y));
    let iz = i32(floor(z));
    let fx = smoothstep3(x - f32(ix));
    let fy = smoothstep3(y - f32(iy));
    let fz = smoothstep3(z - f32(iz));

    let c000 = hash_f(ix, iy, iz, seed);
    let c100 = hash_f(ix + 1, iy, iz, seed);
    let c010 = hash_f(ix, iy + 1, iz, seed);
    let c110 = hash_f(ix + 1, iy + 1, iz, seed);
    let c001 = hash_f(ix, iy, iz + 1, seed);
    let c101 = hash_f(ix + 1, iy, iz + 1, seed);
    let c011 = hash_f(ix, iy + 1, iz + 1, seed);
    let c111 = hash_f(ix + 1, iy + 1, iz + 1, seed);

    let x0 = mix(c000, c100, fx);
    let x1 = mix(c010, c110, fx);
    let x2 = mix(c001, c101, fx);
    let x3 = mix(c011, c111, fx);
    let y0 = mix(x0, x1, fy);
    let y1 = mix(x2, x3, fy);
    return mix(y0, y1, fz);
}

fn fbm3d(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
    var value = 0.0;
    var amp = 1.0;
    var freq = 1.0;
    var max_amp = 0.0;
    for (var i = 0u; i < octaves; i++) {
        value += noise3d(x * freq, y * freq, z * freq, seed + i * 31u) * amp;
        max_amp += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    return value / max_amp;
}

// --- coarse material + density ---

// Palette RGB matching worldgen.rs PALETTE (packed R | G<<8 | B<<16)
const RGB_GRASS: u32      = 0x00994Cu; // [76, 153, 0]
const RGB_DIRT: u32       = 0x2B5A8Bu; // [139, 90, 43]
const RGB_STONE: u32      = 0x808080u; // [128, 128, 128]
const RGB_DARK_STONE: u32 = 0x5A5050u; // [80, 80, 90]
const RGB_WATER: u32      = 0xB4641Eu; // [30, 100, 180]

fn sample_coarse(wx: f32, wy: f32, wz: f32) -> u32 {
    let terrain_noise = fbm3d(wx * 0.012, wy * 0.03, wz * 0.012, 3u, params.seed) - 0.5;
    let density = (params.terrain_base - wy) / params.terrain_amp + terrain_noise;

    if density <= 0.0 {
        if wy > params.water_level {
            return 0u;
        }
        return RGB_WATER | (0xFFu << 24u);
    }

    let cave_a = fbm3d(wx * 0.08, wy * 0.08, wz * 0.08, 2u, params.seed + 777u);
    let cave_b = fbm3d(wx * 0.08, wy * 0.1, wz * 0.08, 2u, params.seed + 1234u);
    if cave_a > params.cave_threshold && cave_b > params.cave_threshold && density < 1.5 {
        if wy <= params.water_level {
            return RGB_WATER | (0xFFu << 24u);
        }
        return 0u;
    }

    let alpha = u32(clamp(density * 8.0, 0.0, 1.0) * 255.0);

    var rgb: u32;
    if density < 0.08 {
        rgb = RGB_GRASS;
    } else if density < 0.25 {
        rgb = RGB_DIRT;
    } else if wy > -5.0 {
        rgb = RGB_STONE;
    } else {
        rgb = RGB_DARK_STONE;
    }

    return rgb | (alpha << 24u);
}

// --- entry point ---

@compute @workgroup_size(73, 1, 1)
fn cs_generate_coarse(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let brick_idx = wg_id.x;
    let grid_pos = requests[brick_idx].xyz;
    let brick_min = params.world_min
        + vec3<f32>(f32(grid_pos.x), f32(grid_pos.y), f32(grid_pos.z)) * params.brick_size;
    let bs = params.brick_size;

    // Map linear index to sub-block position and size
    var sub_center: vec3<f32>;

    if li < 64u {
        // Level 2: 4³ sub-blocks
        let edge = 4u;
        let sub_size = bs / f32(edge);
        let sx = li % edge;
        let sy = (li / edge) % edge;
        let sz = li / (edge * edge);
        sub_center = brick_min + vec3<f32>(
            (f32(sx) + 0.5) * sub_size,
            (f32(sy) + 0.5) * sub_size,
            (f32(sz) + 0.5) * sub_size,
        );
    } else if li < 72u {
        // Level 3: 2³ sub-blocks
        let idx = li - 64u;
        let edge = 2u;
        let sub_size = bs / f32(edge);
        let sx = idx % edge;
        let sy = (idx / edge) % edge;
        let sz = idx / (edge * edge);
        sub_center = brick_min + vec3<f32>(
            (f32(sx) + 0.5) * sub_size,
            (f32(sy) + 0.5) * sub_size,
            (f32(sz) + 0.5) * sub_size,
        );
    } else {
        // Level 4: 1³ — brick center
        sub_center = brick_min + vec3<f32>(bs * 0.5);
    }

    let packed = sample_coarse(sub_center.x, sub_center.y, sub_center.z);
    output[brick_idx * COARSE_MIP_WORDS + li] = packed;
}
