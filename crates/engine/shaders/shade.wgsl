// Deferred shade: full-screen compute shader evaluating PBR lighting.
// Reads GBuffer (depth, albedo, normal, material), light SSBO,
// shadow atlas, and clustered light grid. Outputs HDR color.

// ---- GBuffer inputs (group 0) ----

@group(0) @binding(0) var gbuf_depth: texture_depth_2d;
@group(0) @binding(1) var gbuf_albedo: texture_2d<f32>;
@group(0) @binding(2) var gbuf_normal: texture_2d<f32>;
@group(0) @binding(3) var gbuf_material: texture_2d<f32>;

// ---- Camera + lights (group 1) ----

struct ShadeUniforms {
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    screen_size: vec4<f32>,
    near_far: vec4<f32>,
}

struct GpuLight {
    position_range: vec4<f32>,
    direction_type: vec4<f32>,
    color_intensity: vec4<f32>,
    spot_params: vec4<f32>,
}

struct LightHeader {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(1) @binding(0) var<uniform> uniforms: ShadeUniforms;
@group(1) @binding(1) var<uniform> light_header: LightHeader;
@group(1) @binding(2) var<storage, read> lights: array<GpuLight>;

// ---- Shadow atlas (group 2) ----

struct ShadowView {
    view_proj: mat4x4<f32>,
    viewport: vec4<f32>,
}

struct ShadowHeader {
    count: u32,
    atlas_size: f32,
    _pad0: u32,
    _pad1: u32,
}

@group(2) @binding(0) var shadow_atlas: texture_depth_2d;
@group(2) @binding(1) var<uniform> shadow_header: ShadowHeader;
@group(2) @binding(2) var<storage, read> shadow_views: array<ShadowView>;

// ---- Cluster grid (group 3) ----

struct ClusterParams {
    tiles_x: u32,
    tiles_y: u32,
    num_slices: u32,
    tile_size: u32,
    near: f32,
    log_ratio: f32,
    _pad0: u32,
    _pad1: u32,
}

@group(3) @binding(0) var<uniform> cluster_params: ClusterParams;
@group(3) @binding(1) var<storage, read> cluster_offsets: array<u32>;
@group(3) @binding(2) var<storage, read> cluster_counts: array<u32>;
@group(3) @binding(3) var<storage, read> light_indices: array<u32>;

// ---- HDR output ----

@group(0) @binding(4) var hdr_output: texture_storage_2d<rgba16float, write>;

// ---- Constants ----

const PI: f32 = 3.14159265359;
const DIELECTRIC_F0: f32 = 0.04;

// ---- Normal decoding ----

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    let p = e * 2.0 - 1.0;
    var n = vec3<f32>(p.x, p.y, 1.0 - abs(p.x) - abs(p.y));
    if n.z < 0.0 {
        let sign_x = select(-1.0, 1.0, n.x >= 0.0);
        let sign_y = select(-1.0, 1.0, n.y >= 0.0);
        let tmp = n.xy;
        n.x = (1.0 - abs(tmp.y)) * sign_x;
        n.y = (1.0 - abs(tmp.x)) * sign_y;
    }
    return normalize(n);
}

// ---- Position reconstruction ----

fn reconstruct_world_pos(pixel: vec2<u32>, depth: f32) -> vec3<f32> {
    let screen_size = uniforms.screen_size.xy;
    let uv = (vec2<f32>(pixel) + 0.5) / screen_size;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
    let world_h = uniforms.inv_view_proj * ndc;
    return world_h.xyz / world_h.w;
}

// ---- Cook-Torrance BRDF ----

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let denom = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom * denom);
}

fn geometry_smith_ggx(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    let g1_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let g1_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return g1_v * g1_l;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(saturate(1.0 - cos_theta), 5.0);
}

fn cook_torrance(
    n: vec3<f32>, v: vec3<f32>, l: vec3<f32>,
    albedo: vec3<f32>, roughness: f32, metallic: f32,
) -> vec3<f32> {
    let h = normalize(v + l);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 0.001);
    let n_dot_h = max(dot(n, h), 0.0);
    let h_dot_v = max(dot(h, v), 0.0);

    let f0 = mix(vec3<f32>(DIELECTRIC_F0), albedo, metallic);

    let d = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith_ggx(n_dot_v, n_dot_l, roughness);
    let f = fresnel_schlick(h_dot_v, f0);

    let spec = (d * g * f) / (4.0 * n_dot_v * n_dot_l + 0.0001);

    let k_s = f;
    let k_d = (vec3<f32>(1.0) - k_s) * (1.0 - metallic);
    let diffuse = k_d * albedo / PI;

    return (diffuse + spec) * n_dot_l;
}

// ---- Attenuation ----

fn point_attenuation(distance: f32, range: f32) -> f32 {
    let ratio = distance / range;
    let ratio2 = ratio * ratio;
    let ratio4 = ratio2 * ratio2;
    let falloff = saturate(1.0 - ratio4);
    return (falloff * falloff) / (distance * distance + 1.0);
}

fn spot_attenuation(cos_angle: f32, inner_cos: f32, outer_cos: f32) -> f32 {
    return smoothstep(outer_cos, inner_cos, cos_angle);
}

// ---- Shadow sampling ----

fn sample_shadow(world_pos: vec3<f32>, shadow_idx: i32) -> f32 {
    if shadow_idx < 0 || shadow_idx >= i32(shadow_header.count) {
        return 1.0;
    }
    let sv = shadow_views[shadow_idx];
    let light_clip = sv.view_proj * vec4<f32>(world_pos, 1.0);
    let light_ndc = light_clip.xyz / light_clip.w;

    if any(light_ndc.xy < vec2<f32>(-1.0)) || any(light_ndc.xy > vec2<f32>(1.0)) {
        return 1.0;
    }

    let atlas_size = shadow_header.atlas_size;
    let vp = sv.viewport;
    let shadow_uv = light_ndc.xy * 0.5 + 0.5;
    let atlas_pixel = vec2<i32>(
        i32(vp.x + shadow_uv.x * vp.z),
        i32(vp.y + (1.0 - shadow_uv.y) * vp.w),
    );

    let shadow_depth = textureLoad(shadow_atlas, atlas_pixel, 0);
    return select(0.0, 1.0, light_ndc.z <= shadow_depth);
}

// ---- Cluster lookup ----

fn get_cluster_index(pixel: vec2<u32>, depth: f32) -> u32 {
    let tile_x = pixel.x / cluster_params.tile_size;
    let tile_y = pixel.y / cluster_params.tile_size;

    let linear_depth = uniforms.near_far.x * uniforms.near_far.y
        / (uniforms.near_far.y - depth * (uniforms.near_far.y - uniforms.near_far.x));

    let slice = u32(max(log(linear_depth / cluster_params.near) * cluster_params.log_ratio, 0.0));
    let clamped_slice = min(slice, cluster_params.num_slices - 1u);

    return tile_x + cluster_params.tiles_x * (tile_y + cluster_params.tiles_y * clamped_slice);
}

// ---- Main ----

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.xy;
    let screen = vec2<u32>(uniforms.screen_size.xy);
    if pixel.x >= screen.x || pixel.y >= screen.y {
        return;
    }

    let depth = textureLoad(gbuf_depth, pixel, 0);
    if depth >= 1.0 {
        textureStore(hdr_output, pixel, vec4<f32>(0.0, 0.0, 0.0, 1.0));
        return;
    }

    let albedo_raw = textureLoad(gbuf_albedo, pixel, 0);
    let normal_raw = textureLoad(gbuf_normal, pixel, 0);
    let material_raw = textureLoad(gbuf_material, pixel, 0);

    let albedo = albedo_raw.rgb;
    let normal = oct_decode(normal_raw.xy);
    let roughness = max(material_raw.x, 0.04);
    let metallic = material_raw.y;

    let world_pos = reconstruct_world_pos(pixel, depth);
    let v = normalize(uniforms.camera_pos.xyz - world_pos);

    let ambient = vec3<f32>(0.03) * albedo * (1.0 - metallic * 0.5);

    var color = ambient;

    let cluster_idx = get_cluster_index(pixel, depth);
    let offset = cluster_offsets[cluster_idx];
    let count = cluster_counts[cluster_idx];

    for (var i = 0u; i < count; i++) {
        let light_idx = light_indices[offset + i];
        let light = lights[light_idx];
        let light_type = u32(light.direction_type.w);
        let intensity = light.color_intensity.w;
        let light_color = light.color_intensity.xyz;
        let shadow_idx = i32(light.spot_params.z);

        var l: vec3<f32>;
        var attenuation: f32 = 1.0;

        if light_type == 0u {
            // Directional
            l = -normalize(light.direction_type.xyz);
        } else if light_type == 1u {
            // Point
            let to_light = light.position_range.xyz - world_pos;
            let dist = length(to_light);
            l = to_light / max(dist, 0.0001);
            attenuation = point_attenuation(dist, light.position_range.w);
        } else {
            // Spot
            let to_light = light.position_range.xyz - world_pos;
            let dist = length(to_light);
            l = to_light / max(dist, 0.0001);
            let cos_angle = dot(-l, normalize(light.direction_type.xyz));
            attenuation = point_attenuation(dist, light.position_range.w)
                * spot_attenuation(cos_angle, light.spot_params.x, light.spot_params.y);
        }

        let shadow = sample_shadow(world_pos, shadow_idx);
        let brdf = cook_torrance(normal, v, l, albedo, roughness, metallic);
        color += brdf * light_color * intensity * attenuation * shadow;
    }

    textureStore(hdr_output, pixel, vec4<f32>(color, 1.0));
}
