// GPU terrain generator — direct port of worldgen.rs noise + material logic.

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

const BRICK_EDGE: u32 = 16u;
const VOXEL_SCALE: f32 = 0.1;
const WORDS_PER_BRICK: u32 = 1024u;

const MAT_AIR: u32 = 0u;
const MAT_GRASS: u32 = 1u;
const MAT_DIRT: u32 = 2u;
const MAT_STONE: u32 = 3u;
const MAT_DARK_STONE: u32 = 4u;
const MAT_WATER: u32 = 5u;
const MAT_GRASS_ALT: u32 = 8u;

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

fn sample_material(wx: f32, wy: f32, wz: f32) -> u32 {
    let terrain_noise = fbm3d(wx * 0.012, wy * 0.03, wz * 0.012, 3u, params.seed) - 0.5;
    let density = (params.terrain_base - wy) / params.terrain_amp + terrain_noise;

    if density <= 0.0 {
        if wy > params.water_level {
            return MAT_AIR;
        }
        return MAT_WATER;
    }

    let cave_a = fbm3d(wx * 0.08, wy * 0.08, wz * 0.08, 2u, params.seed + 777u);
    let cave_b = fbm3d(wx * 0.08, wy * 0.1, wz * 0.08, 2u, params.seed + 1234u);
    if cave_a > params.cave_threshold && cave_b > params.cave_threshold && density < 1.5 {
        if wy <= params.water_level {
            return MAT_WATER;
        }
        return MAT_AIR;
    }

    if density < 0.08 {
        let h = wg_hash(i32(wx * 10.0), i32(wy * 10.0), i32(wz * 10.0), params.seed + 2222u);
        if h % 3u == 0u {
            return MAT_GRASS_ALT;
        }
        return MAT_GRASS;
    }
    if density < 0.25 {
        return MAT_DIRT;
    }
    if wy > -5.0 {
        return MAT_STONE;
    }
    return MAT_DARK_STONE;
}

@compute @workgroup_size(256, 1, 1)
fn cs_generate(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let brick_idx = wg_id.x;
    let grid_pos = requests[brick_idx].xyz;
    let brick_min = params.world_min
        + vec3<f32>(f32(grid_pos.x), f32(grid_pos.y), f32(grid_pos.z)) * params.brick_size;

    let base_output = brick_idx * WORDS_PER_BRICK;

    for (var w = 0u; w < 4u; w++) {
        var packed = 0u;
        for (var b = 0u; b < 4u; b++) {
            let voxel_idx = li * 16u + w * 4u + b;
            let lx = voxel_idx % BRICK_EDGE;
            let ly = (voxel_idx / BRICK_EDGE) % BRICK_EDGE;
            let lz = voxel_idx / (BRICK_EDGE * BRICK_EDGE);

            let wx = brick_min.x + (f32(lx) + 0.5) * VOXEL_SCALE;
            let wy = brick_min.y + (f32(ly) + 0.5) * VOXEL_SCALE;
            let wz = brick_min.z + (f32(lz) + 0.5) * VOXEL_SCALE;

            let mat = sample_material(wx, wy, wz);
            packed |= (mat & 0xFFu) << (b * 8u);
        }
        output[base_output + li * 4u + w] = packed;
    }
}
