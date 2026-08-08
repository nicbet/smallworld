// GBuffer pass: renders meshes into albedo + normal + material + emissive +
// velocity + aux targets. Position is reconstructed from depth in the
// lighting shader.

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
}

struct DrawUniforms {
    model: mat4x4<f32>,
    base_color: vec4<f32>,
    roughness_metallic: vec2<f32>,
    material_id: u32,
    _pad: u32,
    emissive: vec4<f32>,
    prev_model: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var<uniform> draw: DrawUniforms;

// Material textures (group 2)
@group(2) @binding(0) var t_albedo: texture_2d<f32>;
@group(2) @binding(1) var t_normal: texture_2d<f32>;
@group(2) @binding(2) var t_roughness_metallic: texture_2d<f32>;
@group(2) @binding(3) var t_emissive: texture_2d<f32>;
@group(2) @binding(4) var t_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_tangent: vec3<f32>,
    @location(2) tangent_w: f32,
    @location(3) uv: vec2<f32>,
    @location(4) cur_pos_clip: vec4<f32>,
    @location(5) prev_pos_clip: vec4<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let world_pos = draw.model * vec4<f32>(in.position, 1.0);
    let prev_world_pos = draw.prev_model * vec4<f32>(in.position, 1.0);
    var out: VertexOutput;
    out.clip_pos = frame.view_proj * world_pos;
    out.world_normal = normalize((draw.model * vec4<f32>(in.normal, 0.0)).xyz);
    out.world_tangent = normalize((draw.model * vec4<f32>(in.tangent.xyz, 0.0)).xyz);
    out.tangent_w = in.tangent.w;
    out.uv = in.uv;
    out.cur_pos_clip = out.clip_pos;
    out.prev_pos_clip = frame.prev_view_proj * prev_world_pos;
    return out;
}

struct GBufferOutput {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) material: vec4<f32>,
    @location(3) emissive: vec4<f32>,
    @location(4) velocity: vec2<f32>,
    @location(5) aux: u32,
}

// Octahedral normal encoding: unit sphere → [0,1]² for Unorm storage.
fn sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        select(-1.0, 1.0, v.x >= 0.0),
        select(-1.0, 1.0, v.y >= 0.0),
    );
}

fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let sum = abs(n.x) + abs(n.y) + abs(n.z);
    var p = n.xy / sum;
    if n.z < 0.0 {
        p = (1.0 - abs(p.yx)) * sign_not_zero(p);
    }
    return p * 0.5 + 0.5;
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) is_front: bool) -> GBufferOutput {
    // Albedo: texture * base_color
    let tex_color = textureSample(t_albedo, t_sampler, in.uv);
    let albedo = tex_color * draw.base_color;

    // Normal: TBN transform from normal map, or vertex normal
    var n = normalize(in.world_normal);
    if !is_front {
        n = -n;
    }
    let t = normalize(in.world_tangent);
    let b = cross(n, t) * in.tangent_w;
    let tbn = mat3x3<f32>(t, b, n);

    let normal_sample = textureSample(t_normal, t_sampler, in.uv).xyz;
    let tangent_normal = normal_sample * 2.0 - 1.0;
    // If the normal map is the flat fallback (0.5, 0.5, 1.0), this produces (0, 0, 1)
    // which transforms to the vertex normal via TBN — correct fallback behavior.
    let world_normal = normalize(tbn * tangent_normal);

    let oct = oct_encode(world_normal);

    // Roughness / metallic: texture channels * scalar
    let rm_sample = textureSample(t_roughness_metallic, t_sampler, in.uv);
    let roughness = rm_sample.g * draw.roughness_metallic.x;
    let metallic = rm_sample.b * draw.roughness_metallic.y;

    // Emissive: texture * scalar factor
    let emissive_sample = textureSample(t_emissive, t_sampler, in.uv);
    let emissive_out = emissive_sample.rgb * draw.emissive.xyz;

    // Velocity: NDC delta between current and previous frame
    let cur_ndc = in.cur_pos_clip.xy / in.cur_pos_clip.w;
    let prev_ndc = in.prev_pos_clip.xy / in.prev_pos_clip.w;

    var out: GBufferOutput;
    out.albedo = albedo;
    out.normal = vec4<f32>(oct, 0.0, 1.0);
    out.material = vec4<f32>(roughness, metallic, 0.0, 0.0);
    out.emissive = vec4<f32>(emissive_out, 1.0);
    out.velocity = cur_ndc - prev_ndc;
    out.aux = draw.material_id & 0x7FFFu;
    return out;
}
