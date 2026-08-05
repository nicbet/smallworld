// Two-level DDA raymarcher over a sparse brick grid.

const EMPTY: u32 = 0xFFFFFFFFu;
const SUN_DIR: vec3<f32> = normalize(vec3<f32>(0.4, 0.8, 0.3));
const SKY_TOP: vec3<f32> = vec3<f32>(0.4, 0.6, 0.9);
const SKY_BOT: vec3<f32> = vec3<f32>(0.7, 0.8, 0.95);
const AMBIENT: f32 = 0.25;
const MAX_COARSE_STEPS: u32 = 512u;
const MAX_FINE_STEPS: u32 = 64u;

struct Uniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    resolution: vec2<f32>,
    _pad0: vec2<f32>,
    world_min: vec3<f32>,
    brick_size: f32,
    grid_dims: vec3<u32>,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> grid_map: array<u32>;
@group(0) @binding(2) var<storage, read> voxels: array<u32>;
@group(0) @binding(3) var<storage, read> palettes: array<u32>;
@group(0) @binding(4) var output: texture_storage_2d<rgba8unorm, write>;

// ---- helpers ----

fn sky(dir: vec3<f32>) -> vec3<f32> {
    let t = clamp(dir.y * 0.5 + 0.5, 0.0, 1.0);
    return mix(SKY_BOT, SKY_TOP, t);
}

fn ray_aabb(ro: vec3<f32>, inv_rd: vec3<f32>, bmin: vec3<f32>, bmax: vec3<f32>) -> vec2<f32> {
    let t0 = (bmin - ro) * inv_rd;
    let t1 = (bmax - ro) * inv_rd;
    let tmin = min(t0, t1);
    let tmax = max(t0, t1);
    return vec2<f32>(
        max(max(tmin.x, tmin.y), tmin.z),
        min(min(tmax.x, tmax.y), tmax.z),
    );
}

fn grid_flat(p: vec3<i32>) -> u32 {
    return u32(p.x) + u.grid_dims.x * (u32(p.y) + u.grid_dims.y * u32(p.z));
}

fn in_grid(p: vec3<i32>) -> bool {
    return all(p >= vec3<i32>(0)) && all(p < vec3<i32>(u.grid_dims));
}

fn read_voxel(handle: u32, idx: u32) -> u32 {
    let word = voxels[handle * WORDS_PER_BRICK + idx / 4u];
    return (word >> ((idx % 4u) * 8u)) & 0xFFu;
}

fn read_palette_color(handle: u32, mat_idx: u32) -> vec3<f32> {
    let packed = palettes[handle * PALETTE_ENTRIES + mat_idx];
    return vec3<f32>(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
    ) / 255.0;
}

const WORDS_PER_BRICK: u32 = BRICK_VOLUME / 4u;
const PALETTE_ENTRIES: u32 = 256u;

// ---- fine DDA inside a 16³ brick ----

struct HitResult {
    hit: bool,
    color: vec3<f32>,
    normal: vec3<f32>,
}

fn trace_brick(
    ro: vec3<f32>, rd: vec3<f32>,
    t_enter: f32,
    handle: u32, brick_min: vec3<f32>,
    entry_normal: vec3<f32>,
) -> HitResult {
    let inv_voxel = 1.0 / VOXEL_SCALE;
    let p_entry = (ro + rd * (t_enter + 0.0005)) - brick_min;
    var voxel = vec3<i32>(floor(p_entry * inv_voxel));
    voxel = clamp(voxel, vec3<i32>(0), vec3<i32>(i32(BRICK_EDGE) - 1));

    let step = vec3<i32>(sign(rd));
    let abs_inv = abs(vec3<f32>(1.0) / (rd * inv_voxel));

    var t_max: vec3<f32>;
    let frac = p_entry * inv_voxel - vec3<f32>(voxel);
    if rd.x > 0.0 { t_max.x = (1.0 - frac.x) * abs_inv.x; } else { t_max.x = frac.x * abs_inv.x; }
    if rd.y > 0.0 { t_max.y = (1.0 - frac.y) * abs_inv.y; } else { t_max.y = frac.y * abs_inv.y; }
    if rd.z > 0.0 { t_max.z = (1.0 - frac.z) * abs_inv.z; } else { t_max.z = frac.z * abs_inv.z; }

    var normal = entry_normal;

    for (var i = 0u; i < MAX_FINE_STEPS; i++) {
        if any(voxel < vec3<i32>(0)) || any(voxel >= vec3<i32>(i32(BRICK_EDGE))) {
            break;
        }

        let idx = u32(voxel.x) + BRICK_EDGE * (u32(voxel.y) + BRICK_EDGE * u32(voxel.z));
        let mat = read_voxel(handle, idx);

        if mat != 0u {
            let base = read_palette_color(handle, mat);
            let ndotl = max(dot(normal, SUN_DIR), 0.0);
            let color = base * (AMBIENT + (1.0 - AMBIENT) * ndotl);
            return HitResult(true, color, normal);
        }

        if t_max.x < t_max.y {
            if t_max.x < t_max.z {
                voxel.x += step.x;
                t_max.x += abs_inv.x;
                normal = vec3<f32>(f32(-step.x), 0.0, 0.0);
            } else {
                voxel.z += step.z;
                t_max.z += abs_inv.z;
                normal = vec3<f32>(0.0, 0.0, f32(-step.z));
            }
        } else {
            if t_max.y < t_max.z {
                voxel.y += step.y;
                t_max.y += abs_inv.y;
                normal = vec3<f32>(0.0, f32(-step.y), 0.0);
            } else {
                voxel.z += step.z;
                t_max.z += abs_inv.z;
                normal = vec3<f32>(0.0, 0.0, f32(-step.z));
            }
        }
    }

    return HitResult(false, vec3<f32>(0.0), vec3<f32>(0.0));
}

// ---- main entry point ----

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let px = gid.xy;
    let res = vec2<u32>(u.resolution);
    if px.x >= res.x || px.y >= res.y {
        return;
    }

    // Pixel → world ray
    let uv = (vec2<f32>(px) + 0.5) / u.resolution;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near_h = u.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far_h  = u.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let near_w = near_h.xyz / near_h.w;
    let far_w  = far_h.xyz / far_h.w;
    let ro = u.camera_pos.xyz;
    let rd = normalize(far_w - near_w);
    let inv_rd = 1.0 / rd;

    // World AABB
    let world_max = u.world_min + vec3<f32>(u.grid_dims) * u.brick_size;
    let world_hit = ray_aabb(ro, inv_rd, u.world_min, world_max);
    let t_entry = max(world_hit.x, 0.0);
    let t_exit  = world_hit.y;

    if t_entry >= t_exit {
        textureStore(output, px, vec4<f32>(sky(rd), 1.0));
        return;
    }

    // Future hooks: transmission (water) and accumulation (fog/god rays)
    var throughput = vec3<f32>(1.0);
    var accumulation = vec3<f32>(0.0);

    // Coarse DDA setup (grid space: one cell = one brick)
    let inv_brick = 1.0 / u.brick_size;
    let p_entry = (ro + rd * (t_entry + 0.001)) - u.world_min;
    var grid_pos = vec3<i32>(floor(p_entry * inv_brick));
    grid_pos = clamp(grid_pos, vec3<i32>(0), vec3<i32>(u.grid_dims) - vec3<i32>(1));

    let step = vec3<i32>(sign(rd));
    let abs_inv_grid = abs(vec3<f32>(1.0) / (rd * inv_brick));

    var t_max_g: vec3<f32>;
    let frac_g = p_entry * inv_brick - vec3<f32>(grid_pos);
    if rd.x > 0.0 { t_max_g.x = (1.0 - frac_g.x) * abs_inv_grid.x; } else { t_max_g.x = frac_g.x * abs_inv_grid.x; }
    if rd.y > 0.0 { t_max_g.y = (1.0 - frac_g.y) * abs_inv_grid.y; } else { t_max_g.y = frac_g.y * abs_inv_grid.y; }
    if rd.z > 0.0 { t_max_g.z = (1.0 - frac_g.z) * abs_inv_grid.z; } else { t_max_g.z = frac_g.z * abs_inv_grid.z; }

    // Initial entry face normal from world AABB
    let tmin_faces = (u.world_min - ro) * inv_rd;
    let tmax_faces = (world_max - ro) * inv_rd;
    let tmin_v = min(tmin_faces, tmax_faces);
    var coarse_normal: vec3<f32>;
    if tmin_v.x > tmin_v.y && tmin_v.x > tmin_v.z {
        coarse_normal = vec3<f32>(-sign(rd.x), 0.0, 0.0);
    } else if tmin_v.y > tmin_v.z {
        coarse_normal = vec3<f32>(0.0, -sign(rd.y), 0.0);
    } else {
        coarse_normal = vec3<f32>(0.0, 0.0, -sign(rd.z));
    }

    var hit_found = false;
    var final_color = vec3<f32>(0.0);

    for (var c = 0u; c < MAX_COARSE_STEPS; c++) {
        if !in_grid(grid_pos) {
            break;
        }

        let handle = grid_map[grid_flat(grid_pos)];
        if handle != EMPTY {
            let brick_min = u.world_min + vec3<f32>(grid_pos) * u.brick_size;
            let brick_max = brick_min + vec3<f32>(u.brick_size);
            let brick_hit = ray_aabb(ro, inv_rd, brick_min, brick_max);
            let brick_t = max(brick_hit.x, 0.0);

            let result = trace_brick(ro, rd, brick_t, handle, brick_min, coarse_normal);
            if result.hit {
                final_color = accumulation + throughput * result.color;
                hit_found = true;

                // Future: if material is transmissive (water):
                //   throughput *= transmission_color;
                //   accumulation += emission;
                //   (continue instead of break)
                break;
            }
        }

        // Future: per-step fog/god ray accumulation
        // accumulation += sample_fog(grid_pos) * throughput * step_length;

        // Step to next grid cell, track entry face normal
        if t_max_g.x < t_max_g.y {
            if t_max_g.x < t_max_g.z {
                grid_pos.x += step.x;
                t_max_g.x += abs_inv_grid.x;
                coarse_normal = vec3<f32>(f32(-step.x), 0.0, 0.0);
            } else {
                grid_pos.z += step.z;
                t_max_g.z += abs_inv_grid.z;
                coarse_normal = vec3<f32>(0.0, 0.0, f32(-step.z));
            }
        } else {
            if t_max_g.y < t_max_g.z {
                grid_pos.y += step.y;
                t_max_g.y += abs_inv_grid.y;
                coarse_normal = vec3<f32>(0.0, f32(-step.y), 0.0);
            } else {
                grid_pos.z += step.z;
                t_max_g.z += abs_inv_grid.z;
                coarse_normal = vec3<f32>(0.0, 0.0, f32(-step.z));
            }
        }
    }

    if !hit_found {
        final_color = sky(rd);
    }

    textureStore(output, px, vec4<f32>(final_color, 1.0));
}
