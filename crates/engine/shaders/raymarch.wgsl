// Two-level DDA raymarcher: dense voxel grid + instanced voxel volumes.

const EMPTY: u32 = 0xFFFFFFFFu;
const SUN_DIR: vec3<f32> = normalize(vec3<f32>(0.4, 0.8, 0.3));
const SKY_TOP: vec3<f32> = vec3<f32>(0.4, 0.6, 0.9);
const SKY_BOT: vec3<f32> = vec3<f32>(0.7, 0.8, 0.95);
const AMBIENT: f32 = 0.25;
const MAX_COARSE_STEPS: u32 = 512u;
const MAX_FINE_STEPS: u32 = 64u;
const SHADOW_BIAS: f32 = 0.01;

const FLAG_SHADOWS: u32 = 1u;
const FLAG_SMOOTH_NORMALS: u32 = 2u;

struct Uniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    resolution: vec2<f32>,
    _pad0: vec2<f32>,
    world_min: vec3<f32>,
    brick_size: f32,
    grid_dims: vec3<u32>,
    flags: u32,
    instance_count: u32,
    focal_length: f32,
    sse_threshold: f32,
    _pad1c: u32,
}

struct ObjectInstance {
    transform: mat4x4<f32>,
    inv_transform: mat4x4<f32>,
    aabb_min: vec4<f32>,
    aabb_max: vec4<f32>,
    grid_dims: vec4<u32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> grid_map: array<u32>;
@group(0) @binding(2) var<storage, read> voxels: array<u32>;
@group(0) @binding(3) var<storage, read> palettes: array<u32>;
@group(0) @binding(4) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(5) var<storage, read> instances: array<ObjectInstance>;
@group(0) @binding(6) var<storage, read> object_grids: array<u32>;
@group(0) @binding(8) var<storage, read> mips: array<u32>;

struct BvhNode {
    aabb_min: vec3<f32>,
    left_or_first: u32,
    aabb_max: vec3<f32>,
    count: u32,
}

@group(0) @binding(7) var<storage, read> bvh_nodes: array<BvhNode>;
@group(0) @binding(9) var<storage, read> coarse_mips: array<u32>;

// ---- helpers ----

const WORDS_PER_BRICK: u32 = BRICK_VOLUME / 4u;
const PALETTE_ENTRIES: u32 = 256u;

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

fn read_voxel(handle: u32, idx: u32) -> u32 {
    let word = voxels[handle * WORDS_PER_BRICK + idx / 4u];
    return (word >> ((idx % 4u) * 8u)) & 0xFFu;
}

fn voxel_idx(v: vec3<i32>) -> u32 {
    return u32(v.x) + BRICK_EDGE * (u32(v.y) + BRICK_EDGE * u32(v.z));
}

fn read_palette_color(handle: u32, mat_idx: u32) -> vec3<f32> {
    let packed = palettes[handle * PALETTE_ENTRIES + mat_idx];
    return vec3<f32>(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
    ) / 255.0;
}

fn is_solid(handle: u32, v: vec3<i32>) -> f32 {
    if any(v < vec3<i32>(0)) || any(v >= vec3<i32>(i32(BRICK_EDGE))) {
        return 0.0;
    }
    return select(0.0, 1.0, read_voxel(handle, voxel_idx(v)) != 0u);
}

fn smooth_normal(handle: u32, v: vec3<i32>, face_normal: vec3<f32>) -> vec3<f32> {
    let gx = is_solid(handle, v - vec3(1,0,0)) - is_solid(handle, v + vec3(1,0,0));
    let gy = is_solid(handle, v - vec3(0,1,0)) - is_solid(handle, v + vec3(0,1,0));
    let gz = is_solid(handle, v - vec3(0,0,1)) - is_solid(handle, v + vec3(0,0,1));
    let grad = vec3<f32>(gx, gy, gz);
    let len = length(grad);
    if len > 0.001 {
        return grad / len;
    }
    return face_normal;
}

// ---- generic hit result ----

struct HitResult {
    hit: bool,
    base_color: vec3<f32>,
    normal: vec3<f32>,
    voxel: vec3<i32>,
    handle: u32,
    world_pos: vec3<f32>,
    t: f32,
}

fn no_hit() -> HitResult {
    return HitResult(false, vec3(0.0), vec3(0.0), vec3(0), 0u, vec3(0.0), 1e20);
}

// ---- fine DDA inside a 16³ brick (parameterised by voxel_scale) ----

fn trace_brick(
    ro: vec3<f32>, rd: vec3<f32>,
    t_enter: f32,
    handle: u32, brick_min: vec3<f32>,
    entry_normal: vec3<f32>,
    vs: f32,
) -> HitResult {
    let inv_voxel = 1.0 / vs;
    let p_entry = (ro + rd * (t_enter + vs * 0.005)) - brick_min;
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

        let mat = read_voxel(handle, voxel_idx(voxel));

        if mat != 0u {
            let base = read_palette_color(handle, mat);
            let wp = brick_min + (vec3<f32>(voxel) + 0.5) * vs;
            return HitResult(true, base, normal, voxel, handle, wp, t_enter);
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

    return no_hit();
}

fn sample_brick_mip(handle: u32, local_uv: vec3<f32>, lod: u32) -> vec4<f32> {
    let offsets = array<u32, 4>(0u, 512u, 576u, 584u);
    let edges = array<u32, 4>(8u, 4u, 2u, 1u);
    let lvl = clamp(lod, 1u, 4u) - 1u;
    let edge = edges[lvl];
    let mip_pos = clamp(
        vec3<i32>(floor(local_uv * f32(edge))),
        vec3<i32>(0),
        vec3<i32>(i32(edge) - 1),
    );
    let flat = u32(mip_pos.x) + edge * (u32(mip_pos.y) + edge * u32(mip_pos.z));
    let packed = mips[handle * MIP_WORDS_PER_BRICK + offsets[lvl] + flat];
    return vec4<f32>(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu),
    ) / 255.0;
}

fn sample_coarse_mip(cell_flat: u32, local_uv: vec3<f32>, lod: u32) -> vec4<f32> {
    let base = cell_flat * COARSE_MIP_WORDS;
    let offsets = array<u32, 3>(0u, 64u, 72u);
    let edges = array<u32, 3>(4u, 2u, 1u);
    let lvl = clamp(lod, 2u, 4u) - 2u;
    let edge = edges[lvl];
    let mip_pos = clamp(
        vec3<i32>(floor(local_uv * f32(edge))),
        vec3<i32>(0),
        vec3<i32>(i32(edge) - 1),
    );
    let flat = u32(mip_pos.x) + edge * (u32(mip_pos.y) + edge * u32(mip_pos.z));
    let packed = coarse_mips[base + offsets[lvl] + flat];
    return vec4<f32>(
        f32(packed & 0xFFu),
        f32((packed >> 8u) & 0xFFu),
        f32((packed >> 16u) & 0xFFu),
        f32((packed >> 24u) & 0xFFu),
    ) / 255.0;
}

// ---- dense grid traversal ----

fn trace_grid(ro: vec3<f32>, rd: vec3<f32>, max_t: f32) -> HitResult {
    let inv_rd = 1.0 / rd;
    let world_max = u.world_min + vec3<f32>(u.grid_dims) * u.brick_size;
    let world_hit = ray_aabb(ro, inv_rd, u.world_min, world_max);
    let t_entry = max(world_hit.x, 0.0);

    if t_entry >= world_hit.y || t_entry >= max_t {
        return no_hit();
    }

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

    let grid_vs = VOXEL_SCALE;

    for (var c = 0u; c < MAX_COARSE_STEPS; c++) {
        if !all(grid_pos >= vec3<i32>(0)) || !all(grid_pos < vec3<i32>(u.grid_dims)) {
            break;
        }

        let flat = u32(grid_pos.x) + u.grid_dims.x * (u32(grid_pos.y) + u.grid_dims.y * u32(grid_pos.z));
        let handle = grid_map[flat];
        if handle != EMPTY {
            let brick_min = u.world_min + vec3<f32>(grid_pos) * u.brick_size;
            let brick_max = brick_min + vec3<f32>(u.brick_size);
            let brick_hit = ray_aabb(ro, inv_rd, brick_min, brick_max);
            let brick_t = max(brick_hit.x, 0.0);

            if brick_t < max_t {
                let dist = max(brick_t, 0.001);
                let sse = grid_vs * u.focal_length / dist;
                if sse < u.sse_threshold {
                    let ratio = u.sse_threshold / max(sse, 0.001);
                    let lod = clamp(u32(ceil(log2(ratio))), 1u, 4u);
                    let entry = (ro + rd * (brick_t + grid_vs * 0.005) - brick_min) / u.brick_size;
                    let local_uv = clamp(entry, vec3(0.001), vec3(0.999));
                    let mip_color = sample_brick_mip(handle, local_uv, lod);
                    if mip_color.a > 0.0 {
                        let wp = ro + rd * brick_t;
                        return HitResult(true, mip_color.rgb, coarse_normal, vec3<i32>(0), handle, wp, brick_t);
                    }
                }
                let result = trace_brick(ro, rd, brick_t, handle, brick_min, coarse_normal, grid_vs);
                if result.hit {
                    return result;
                }
            }
        } else {
            let brick_min = u.world_min + vec3<f32>(grid_pos) * u.brick_size;
            let brick_max = brick_min + vec3<f32>(u.brick_size);
            let brick_hit = ray_aabb(ro, inv_rd, brick_min, brick_max);
            let brick_t = max(brick_hit.x, 0.0);

            if brick_t < max_t {
                let top_mip = coarse_mips[flat * COARSE_MIP_WORDS + 72u];
                if top_mip != 0u {
                    let dist = max(brick_t, 0.001);
                    let sse = grid_vs * u.focal_length / dist;
                    let ratio = u.sse_threshold / max(sse, 0.001);
                    let lod = clamp(u32(ceil(log2(ratio))), 2u, 4u);
                    let entry = (ro + rd * (brick_t + grid_vs * 0.005) - brick_min) / u.brick_size;
                    let local_uv = clamp(entry, vec3(0.001), vec3(0.999));
                    let mip_color = sample_coarse_mip(flat, local_uv, lod);
                    if mip_color.a > 0.0 {
                        let wp = ro + rd * brick_t;
                        return HitResult(true, mip_color.rgb, coarse_normal, vec3<i32>(0), 0u, wp, brick_t);
                    }
                }
            }
        }

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

    return no_hit();
}

// ---- instanced volume traversal ----

fn trace_instance(obj: ObjectInstance, ro: vec3<f32>, rd: vec3<f32>, max_t: f32) -> HitResult {
    let obj_ro = (obj.inv_transform * vec4<f32>(ro, 1.0)).xyz;
    let obj_rd_raw = (obj.inv_transform * vec4<f32>(rd, 0.0)).xyz;
    let rd_scale = length(obj_rd_raw);
    if rd_scale < 1e-10 { return no_hit(); }
    let obj_rd = obj_rd_raw / rd_scale;

    let obj_vs = obj.aabb_min.w;
    let obj_brick_size = f32(BRICK_EDGE) * obj_vs;
    let grid_dims = vec3<u32>(obj.grid_dims.xyz);
    let grid_offset = bitcast<u32>(obj.aabb_max.w);

    let obj_max = vec3<f32>(grid_dims) * obj_brick_size;
    let inv_obj_rd = 1.0 / obj_rd;
    let obj_hit = ray_aabb(obj_ro, inv_obj_rd, vec3<f32>(0.0), obj_max);
    let obj_t_entry = max(obj_hit.x, 0.0);

    if obj_t_entry >= obj_hit.y {
        return no_hit();
    }

    // Check if this object could beat max_t (convert obj t to world t)
    if obj_t_entry / rd_scale >= max_t {
        return no_hit();
    }

    let inv_brick = 1.0 / obj_brick_size;
    let p_entry = obj_ro + obj_rd * (obj_t_entry + obj_vs * 0.005);
    var grid_pos = vec3<i32>(floor(p_entry * inv_brick));
    grid_pos = clamp(grid_pos, vec3<i32>(0), vec3<i32>(grid_dims) - vec3<i32>(1));

    let step = vec3<i32>(sign(obj_rd));
    let abs_inv_grid = abs(vec3<f32>(1.0) / (obj_rd * inv_brick));

    var t_max_g: vec3<f32>;
    let frac_g = p_entry * inv_brick - vec3<f32>(grid_pos);
    if obj_rd.x > 0.0 { t_max_g.x = (1.0 - frac_g.x) * abs_inv_grid.x; } else { t_max_g.x = frac_g.x * abs_inv_grid.x; }
    if obj_rd.y > 0.0 { t_max_g.y = (1.0 - frac_g.y) * abs_inv_grid.y; } else { t_max_g.y = frac_g.y * abs_inv_grid.y; }
    if obj_rd.z > 0.0 { t_max_g.z = (1.0 - frac_g.z) * abs_inv_grid.z; } else { t_max_g.z = frac_g.z * abs_inv_grid.z; }

    // Entry normal
    let tmin_v = min(-obj_ro * inv_obj_rd, (obj_max - obj_ro) * inv_obj_rd);
    var coarse_normal: vec3<f32>;
    if tmin_v.x > tmin_v.y && tmin_v.x > tmin_v.z {
        coarse_normal = vec3<f32>(-sign(obj_rd.x), 0.0, 0.0);
    } else if tmin_v.y > tmin_v.z {
        coarse_normal = vec3<f32>(0.0, -sign(obj_rd.y), 0.0);
    } else {
        coarse_normal = vec3<f32>(0.0, 0.0, -sign(obj_rd.z));
    }

    for (var c = 0u; c < MAX_COARSE_STEPS; c++) {
        if !all(grid_pos >= vec3<i32>(0)) || !all(grid_pos < vec3<i32>(grid_dims)) {
            break;
        }

        let flat = u32(grid_pos.x) + grid_dims.x * (u32(grid_pos.y) + grid_dims.y * u32(grid_pos.z));
        let handle = object_grids[grid_offset + flat];
        if handle != EMPTY {
            let brick_min = vec3<f32>(grid_pos) * obj_brick_size;
            let brick_max = brick_min + vec3<f32>(obj_brick_size);
            let brick_hit = ray_aabb(obj_ro, inv_obj_rd, brick_min, brick_max);
            let brick_t = max(brick_hit.x, 0.0);

            let world_brick_t = brick_t / rd_scale;
            let world_dist = max(world_brick_t, 0.001);
            let sse = obj_vs * u.focal_length / world_dist;
            if sse < u.sse_threshold {
                let ratio = u.sse_threshold / max(sse, 0.001);
                let lod = clamp(u32(ceil(log2(ratio))), 1u, 4u);
                let entry = (obj_ro + obj_rd * (brick_t + obj_vs * 0.005) - brick_min) / obj_brick_size;
                let local_uv = clamp(entry, vec3(0.001), vec3(0.999));
                let mip_color = sample_brick_mip(handle, local_uv, lod);
                if mip_color.a > 0.0 {
                    let obj_wp = obj_ro + obj_rd * brick_t;
                    let world_pos = (obj.transform * vec4<f32>(obj_wp, 1.0)).xyz;
                    let world_normal = normalize((obj.transform * vec4<f32>(coarse_normal, 0.0)).xyz);
                    return HitResult(true, mip_color.rgb, world_normal, vec3<i32>(0), handle, world_pos, world_brick_t);
                }
            }
            let result = trace_brick(obj_ro, obj_rd, brick_t, handle, brick_min, coarse_normal, obj_vs);
            if result.hit {
                let world_pos = (obj.transform * vec4<f32>(result.world_pos, 1.0)).xyz;
                let world_normal = normalize((obj.transform * vec4<f32>(result.normal, 0.0)).xyz);
                let world_t = result.t / rd_scale;
                return HitResult(true, result.base_color, world_normal, result.voxel, result.handle, world_pos, world_t);
            }
        }

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

    return no_hit();
}

// ---- shadow ray (grid + instances) ----

fn any_hit_brick(ro: vec3<f32>, rd: vec3<f32>, t_enter: f32, handle: u32, brick_min: vec3<f32>, vs: f32) -> bool {
    let inv_voxel = 1.0 / vs;
    let p_entry = (ro + rd * (t_enter + vs * 0.005)) - brick_min;
    var voxel = vec3<i32>(floor(p_entry * inv_voxel));
    voxel = clamp(voxel, vec3<i32>(0), vec3<i32>(i32(BRICK_EDGE) - 1));

    let step = vec3<i32>(sign(rd));
    let abs_inv = abs(vec3<f32>(1.0) / (rd * inv_voxel));

    var t_max_f: vec3<f32>;
    let frac = p_entry * inv_voxel - vec3<f32>(voxel);
    if rd.x > 0.0 { t_max_f.x = (1.0 - frac.x) * abs_inv.x; } else { t_max_f.x = frac.x * abs_inv.x; }
    if rd.y > 0.0 { t_max_f.y = (1.0 - frac.y) * abs_inv.y; } else { t_max_f.y = frac.y * abs_inv.y; }
    if rd.z > 0.0 { t_max_f.z = (1.0 - frac.z) * abs_inv.z; } else { t_max_f.z = frac.z * abs_inv.z; }

    for (var i = 0u; i < MAX_FINE_STEPS; i++) {
        if any(voxel < vec3<i32>(0)) || any(voxel >= vec3<i32>(i32(BRICK_EDGE))) { break; }
        if read_voxel(handle, voxel_idx(voxel)) != 0u { return true; }
        if t_max_f.x < t_max_f.y {
            if t_max_f.x < t_max_f.z { voxel.x += step.x; t_max_f.x += abs_inv.x; }
            else { voxel.z += step.z; t_max_f.z += abs_inv.z; }
        } else {
            if t_max_f.y < t_max_f.z { voxel.y += step.y; t_max_f.y += abs_inv.y; }
            else { voxel.z += step.z; t_max_f.z += abs_inv.z; }
        }
    }
    return false;
}

fn trace_shadow(ro: vec3<f32>, rd: vec3<f32>) -> bool {
    // Test grid
    let inv_rd = 1.0 / rd;
    let world_max = u.world_min + vec3<f32>(u.grid_dims) * u.brick_size;
    let world_hit = ray_aabb(ro, inv_rd, u.world_min, world_max);
    let t_entry = max(world_hit.x, 0.0);

    if t_entry < world_hit.y {
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

        for (var c = 0u; c < MAX_COARSE_STEPS; c++) {
            if !all(grid_pos >= vec3<i32>(0)) || !all(grid_pos < vec3<i32>(u.grid_dims)) { break; }
            let flat = u32(grid_pos.x) + u.grid_dims.x * (u32(grid_pos.y) + u.grid_dims.y * u32(grid_pos.z));
            let handle = grid_map[flat];
            if handle != EMPTY {
                let brick_min = u.world_min + vec3<f32>(grid_pos) * u.brick_size;
                let brick_hit = ray_aabb(ro, inv_rd, brick_min, brick_min + vec3(u.brick_size));
                if any_hit_brick(ro, rd, max(brick_hit.x, 0.0), handle, brick_min, VOXEL_SCALE) {
                    return true;
                }
            }
            if t_max_g.x < t_max_g.y {
                if t_max_g.x < t_max_g.z { grid_pos.x += step.x; t_max_g.x += abs_inv_grid.x; }
                else { grid_pos.z += step.z; t_max_g.z += abs_inv_grid.z; }
            } else {
                if t_max_g.y < t_max_g.z { grid_pos.y += step.y; t_max_g.y += abs_inv_grid.y; }
                else { grid_pos.z += step.z; t_max_g.z += abs_inv_grid.z; }
            }
        }
    }

    // Test object instances (BVH)
    if u.instance_count > 0u {
        var stack: array<u32, 32>;
        var sp = 1u;
        stack[0] = 0u;

        while sp > 0u {
            sp -= 1u;
            let node = bvh_nodes[stack[sp]];
            let node_hit = ray_aabb(ro, inv_rd, node.aabb_min, node.aabb_max);
            if max(node_hit.x, 0.0) >= node_hit.y {
                continue;
            }

            if node.count > 0u {
                for (var idx = node.left_or_first; idx < node.left_or_first + node.count; idx++) {
                    let obj = instances[idx];
                    let obj_ro = (obj.inv_transform * vec4<f32>(ro, 1.0)).xyz;
                    let obj_rd = normalize((obj.inv_transform * vec4<f32>(rd, 0.0)).xyz);
                    let obj_vs = obj.aabb_min.w;
                    let obj_brick_size = f32(BRICK_EDGE) * obj_vs;
                    let grid_dims = vec3<u32>(obj.grid_dims.xyz);
                    let grid_offset = bitcast<u32>(obj.aabb_max.w);
                    let obj_max = vec3<f32>(grid_dims) * obj_brick_size;

                    let inv_obj_rd = 1.0 / obj_rd;
                    let local_hit = ray_aabb(obj_ro, inv_obj_rd, vec3(0.0), obj_max);
                    let local_t = max(local_hit.x, 0.0);
                    if local_t >= local_hit.y { continue; }

                    let inv_brick_o = 1.0 / obj_brick_size;
                    let p_entry_o = obj_ro + obj_rd * (local_t + obj_vs * 0.005);
                    var gp = vec3<i32>(floor(p_entry_o * inv_brick_o));
                    gp = clamp(gp, vec3<i32>(0), vec3<i32>(grid_dims) - vec3<i32>(1));

                    let step_o = vec3<i32>(sign(obj_rd));
                    let abs_inv_o = abs(vec3<f32>(1.0) / (obj_rd * inv_brick_o));
                    var tmo: vec3<f32>;
                    let fo = p_entry_o * inv_brick_o - vec3<f32>(gp);
                    if obj_rd.x > 0.0 { tmo.x = (1.0 - fo.x) * abs_inv_o.x; } else { tmo.x = fo.x * abs_inv_o.x; }
                    if obj_rd.y > 0.0 { tmo.y = (1.0 - fo.y) * abs_inv_o.y; } else { tmo.y = fo.y * abs_inv_o.y; }
                    if obj_rd.z > 0.0 { tmo.z = (1.0 - fo.z) * abs_inv_o.z; } else { tmo.z = fo.z * abs_inv_o.z; }

                    for (var c = 0u; c < 128u; c++) {
                        if !all(gp >= vec3<i32>(0)) || !all(gp < vec3<i32>(grid_dims)) { break; }
                        let flat = u32(gp.x) + grid_dims.x * (u32(gp.y) + grid_dims.y * u32(gp.z));
                        let h = object_grids[grid_offset + flat];
                        if h != EMPTY {
                            let bm = vec3<f32>(gp) * obj_brick_size;
                            let bh = ray_aabb(obj_ro, inv_obj_rd, bm, bm + vec3(obj_brick_size));
                            if any_hit_brick(obj_ro, obj_rd, max(bh.x, 0.0), h, bm, obj_vs) {
                                return true;
                            }
                        }
                        if tmo.x < tmo.y {
                            if tmo.x < tmo.z { gp.x += step_o.x; tmo.x += abs_inv_o.x; }
                            else { gp.z += step_o.z; tmo.z += abs_inv_o.z; }
                        } else {
                            if tmo.y < tmo.z { gp.y += step_o.y; tmo.y += abs_inv_o.y; }
                            else { gp.z += step_o.z; tmo.z += abs_inv_o.z; }
                        }
                    }
                }
            } else {
                stack[sp] = node.left_or_first;
                sp += 1u;
                stack[sp] = node.left_or_first + 1u;
                sp += 1u;
            }
        }
    }

    return false;
}

// ---- main entry point ----

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let px = gid.xy;
    let res = vec2<u32>(u.resolution);
    if px.x >= res.x || px.y >= res.y {
        return;
    }

    let shadows_on = (u.flags & FLAG_SHADOWS) != 0u;
    let smooth_on  = (u.flags & FLAG_SMOOTH_NORMALS) != 0u;

    let uv = (vec2<f32>(px) + 0.5) / u.resolution;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near_h = u.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far_h  = u.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let near_w = near_h.xyz / near_h.w;
    let far_w  = far_h.xyz / far_h.w;
    let ro = u.camera_pos.xyz;
    let rd = normalize(far_w - near_w);

    // Trace terrain
    var best = trace_grid(ro, rd, 1e20);

    // Trace object instances (BVH traversal)
    let inv_rd = 1.0 / rd;
    if u.instance_count > 0u {
        var stack: array<u32, 32>;
        var sp = 1u;
        stack[0] = 0u;

        while sp > 0u {
            sp -= 1u;
            let node = bvh_nodes[stack[sp]];
            let node_hit = ray_aabb(ro, inv_rd, node.aabb_min, node.aabb_max);
            if max(node_hit.x, 0.0) >= node_hit.y || max(node_hit.x, 0.0) >= best.t {
                continue;
            }

            if node.count > 0u {
                // Leaf: test instances
                for (var i = node.left_or_first; i < node.left_or_first + node.count; i++) {
                    let obj = instances[i];
                    let obj_hit = trace_instance(obj, ro, rd, best.t);
                    if obj_hit.hit && obj_hit.t < best.t {
                        best = obj_hit;
                    }
                }
            } else {
                // Internal: push children
                stack[sp] = node.left_or_first;
                sp += 1u;
                stack[sp] = node.left_or_first + 1u;
                sp += 1u;
            }
        }
    }

    // Shade
    if best.hit {
        var normal = best.normal;
        if smooth_on {
            normal = smooth_normal(best.handle, best.voxel, normal);
        }
        let ndotl = max(dot(normal, SUN_DIR), 0.0);
        var shadow = 1.0;
        if shadows_on {
            let vs_for_bias = VOXEL_SCALE; // approximate, fine for bias
            let shadow_origin = best.world_pos + normal * (vs_for_bias * 0.5 + SHADOW_BIAS);
            if trace_shadow(shadow_origin, SUN_DIR) {
                shadow = 0.0;
            }
        }
        let color = best.base_color * (AMBIENT + (1.0 - AMBIENT) * ndotl * shadow);
        textureStore(output, px, vec4<f32>(color, 1.0));
    } else {
        textureStore(output, px, vec4<f32>(sky(rd), 1.0));
    }
}
