// GBuffer pass: renders meshes into albedo + normal + material targets.
// Position is reconstructed from depth in the lighting shader.

struct FrameUniforms {
    view_proj: mat4x4<f32>,
}

struct DrawUniforms {
    model: mat4x4<f32>,
    base_color: vec4<f32>,
    roughness_metallic: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var<uniform> draw: DrawUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let world_pos = draw.model * vec4<f32>(in.position, 1.0);
    var out: VertexOutput;
    out.clip_pos = frame.view_proj * world_pos;
    out.world_normal = normalize((draw.model * vec4<f32>(in.normal, 0.0)).xyz);
    return out;
}

struct GBufferOutput {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) material: vec4<f32>,
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
fn fs_main(in: VertexOutput) -> GBufferOutput {
    let n = normalize(in.world_normal);
    let oct = oct_encode(n);

    var out: GBufferOutput;
    out.albedo = draw.base_color;
    out.normal = vec4<f32>(oct, 0.0, 1.0);
    out.material = vec4<f32>(draw.roughness_metallic, 0.0, 0.0);
    return out;
}
