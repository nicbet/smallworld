// Fullscreen compute-pass raymarcher for a dense 256³ voxel volume.

const VOLUME_EDGE: u32 = 256u;
const VOLUME_HALF: f32 = f32(VOLUME_EDGE) * VOXEL_SCALE * 0.5; // 12.8

const SUN_DIR: vec3<f32> = normalize(vec3<f32>(0.4, 0.8, 0.3));
const SKY_TOP: vec3<f32> = vec3<f32>(0.4, 0.6, 0.9);
const SKY_BOT: vec3<f32> = vec3<f32>(0.7, 0.8, 0.95);
const AMBIENT: f32 = 0.25;
const MAX_STEPS: u32 = 512u;

struct Camera {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    resolution: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> voxels: array<u32>;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;

fn sky(dir: vec3<f32>) -> vec3<f32> {
    let t = clamp(dir.y * 0.5 + 0.5, 0.0, 1.0);
    return mix(SKY_BOT, SKY_TOP, t);
}

fn material_color(id: u32) -> vec3<f32> {
    switch id {
        case 1u: { return vec3<f32>(0.40, 0.55, 0.30); } // ground
        case 2u: { return vec3<f32>(0.70, 0.65, 0.60); } // stone
        default: { return vec3<f32>(1.00, 0.00, 1.00); } // debug magenta
    }
}

fn voxel_index(p: vec3<u32>) -> u32 {
    return p.x + VOLUME_EDGE * (p.y + VOLUME_EDGE * p.z);
}

fn in_bounds(p: vec3<i32>) -> bool {
    return all(p >= vec3<i32>(0)) && all(p < vec3<i32>(i32(VOLUME_EDGE)));
}

fn ray_aabb(ro: vec3<f32>, inv_rd: vec3<f32>, bmin: vec3<f32>, bmax: vec3<f32>) -> vec2<f32> {
    let t0 = (bmin - ro) * inv_rd;
    let t1 = (bmax - ro) * inv_rd;
    let tmin = min(t0, t1);
    let tmax = max(t0, t1);
    let entry = max(max(tmin.x, tmin.y), tmin.z);
    let exit  = min(min(tmax.x, tmax.y), tmax.z);
    return vec2<f32>(entry, exit);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let px = gid.xy;
    let res = vec2<u32>(camera.resolution);
    if px.x >= res.x || px.y >= res.y {
        return;
    }

    // Pixel → NDC → world ray
    let uv = (vec2<f32>(px) + 0.5) / camera.resolution;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near_h = camera.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far_h  = camera.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let near_w = near_h.xyz / near_h.w;
    let far_w  = far_h.xyz / far_h.w;
    let ro = camera.camera_pos.xyz;
    let rd = normalize(far_w - near_w);

    // Volume bounds in world space (centered at origin)
    let vol_min = vec3<f32>(-VOLUME_HALF);
    let vol_max = vec3<f32>( VOLUME_HALF);

    let inv_rd = 1.0 / rd;
    let hit = ray_aabb(ro, inv_rd, vol_min, vol_max);
    let t_entry = max(hit.x, 0.0);
    let t_exit  = hit.y;

    if t_entry >= t_exit {
        let color = sky(rd);
        textureStore(output, px, vec4<f32>(color, 1.0));
        return;
    }

    // World position → voxel coordinates
    let voxel_scale = VOXEL_SCALE;
    let inv_voxel  = 1.0 / voxel_scale;

    // Entry point in voxel space
    let p_entry = (ro + rd * (t_entry + 0.001)) - vol_min;
    var voxel = vec3<i32>(floor(p_entry * inv_voxel));
    voxel = clamp(voxel, vec3<i32>(0), vec3<i32>(i32(VOLUME_EDGE) - 1));

    // DDA setup in voxel space
    let step = vec3<i32>(sign(rd));
    let rd_vs = rd * inv_voxel; // ray direction in voxel units
    let abs_inv_rd_vs = abs(1.0 / rd_vs);

    // Distance to next voxel boundary along each axis
    var t_max: vec3<f32>;
    if rd.x > 0.0 { t_max.x = (f32(voxel.x + 1) - p_entry.x * inv_voxel) * abs_inv_rd_vs.x; }
    else           { t_max.x = (p_entry.x * inv_voxel - f32(voxel.x))      * abs_inv_rd_vs.x; }
    if rd.y > 0.0 { t_max.y = (f32(voxel.y + 1) - p_entry.y * inv_voxel) * abs_inv_rd_vs.y; }
    else           { t_max.y = (p_entry.y * inv_voxel - f32(voxel.y))      * abs_inv_rd_vs.y; }
    if rd.z > 0.0 { t_max.z = (f32(voxel.z + 1) - p_entry.z * inv_voxel) * abs_inv_rd_vs.z; }
    else           { t_max.z = (p_entry.z * inv_voxel - f32(voxel.z))      * abs_inv_rd_vs.z; }

    let t_delta = abs_inv_rd_vs;

    var normal = vec3<f32>(0.0);
    var hit_found = false;

    for (var i = 0u; i < MAX_STEPS; i++) {
        if !in_bounds(voxel) {
            break;
        }

        let idx = voxel_index(vec3<u32>(voxel));
        let mat = voxels[idx];
        if mat != 0u {
            let base = material_color(mat);
            let ndotl = max(dot(normal, SUN_DIR), 0.0);
            let color = base * (AMBIENT + (1.0 - AMBIENT) * ndotl);
            textureStore(output, px, vec4<f32>(color, 1.0));
            hit_found = true;
            break;
        }

        // Step to next voxel
        if t_max.x < t_max.y {
            if t_max.x < t_max.z {
                voxel.x += step.x;
                t_max.x += t_delta.x;
                normal = vec3<f32>(f32(-step.x), 0.0, 0.0);
            } else {
                voxel.z += step.z;
                t_max.z += t_delta.z;
                normal = vec3<f32>(0.0, 0.0, f32(-step.z));
            }
        } else {
            if t_max.y < t_max.z {
                voxel.y += step.y;
                t_max.y += t_delta.y;
                normal = vec3<f32>(0.0, f32(-step.y), 0.0);
            } else {
                voxel.z += step.z;
                t_max.z += t_delta.z;
                normal = vec3<f32>(0.0, 0.0, f32(-step.z));
            }
        }
    }

    if !hit_found {
        let color = sky(rd);
        textureStore(output, px, vec4<f32>(color, 1.0));
    }
}
