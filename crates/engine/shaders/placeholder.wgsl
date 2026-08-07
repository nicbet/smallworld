// Throwaway placeholder: colored cube, removed when real renderer lands.

struct Uniforms {
    mvp: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // 36 vertices: 6 faces × 2 triangles × 3 verts
    var positions = array<vec3<f32>, 36>(
        // front
        vec3(-1., -1.,  1.), vec3( 1., -1.,  1.), vec3( 1.,  1.,  1.),
        vec3(-1., -1.,  1.), vec3( 1.,  1.,  1.), vec3(-1.,  1.,  1.),
        // back
        vec3( 1., -1., -1.), vec3(-1., -1., -1.), vec3(-1.,  1., -1.),
        vec3( 1., -1., -1.), vec3(-1.,  1., -1.), vec3( 1.,  1., -1.),
        // right
        vec3( 1., -1.,  1.), vec3( 1., -1., -1.), vec3( 1.,  1., -1.),
        vec3( 1., -1.,  1.), vec3( 1.,  1., -1.), vec3( 1.,  1.,  1.),
        // left
        vec3(-1., -1., -1.), vec3(-1., -1.,  1.), vec3(-1.,  1.,  1.),
        vec3(-1., -1., -1.), vec3(-1.,  1.,  1.), vec3(-1.,  1., -1.),
        // top
        vec3(-1.,  1.,  1.), vec3( 1.,  1.,  1.), vec3( 1.,  1., -1.),
        vec3(-1.,  1.,  1.), vec3( 1.,  1., -1.), vec3(-1.,  1., -1.),
        // bottom
        vec3(-1., -1., -1.), vec3( 1., -1., -1.), vec3( 1., -1.,  1.),
        vec3(-1., -1., -1.), vec3( 1., -1.,  1.), vec3(-1., -1.,  1.),
    );
    var colors = array<vec3<f32>, 6>(
        vec3(0.9, 0.2, 0.2),  // front  - red
        vec3(0.2, 0.9, 0.2),  // back   - green
        vec3(0.2, 0.2, 0.9),  // right  - blue
        vec3(0.9, 0.9, 0.2),  // left   - yellow
        vec3(0.9, 0.2, 0.9),  // top    - magenta
        vec3(0.2, 0.9, 0.9),  // bottom - cyan
    );
    var out: VsOut;
    out.pos = u.mvp * vec4(positions[vi], 1.0);
    out.color = colors[vi / 6u];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4(in.color, 1.0);
}
