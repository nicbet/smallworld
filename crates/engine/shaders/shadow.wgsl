// Shadow pass: depth-only rendering from a light's perspective.
// Reuses the same vertex layout as gbuffer.wgsl but writes no color.

struct ShadowUniforms {
    light_view_proj: mat4x4<f32>,
}

struct DrawUniforms {
    model: mat4x4<f32>,
    base_color: vec4<f32>,
    roughness_metallic: vec2<f32>,
    _pad: vec2<f32>,
    emissive: vec4<f32>,
}

@group(0) @binding(0) var<uniform> shadow: ShadowUniforms;
@group(1) @binding(0) var<uniform> draw: DrawUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
}

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    let world_pos = draw.model * vec4<f32>(in.position, 1.0);
    return shadow.light_view_proj * world_pos;
}
