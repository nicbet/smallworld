// HZB builder: downsample depth into a mip chain.
// Each dispatch reads source mip and writes max of each 2x2 block to dest mip.
// Max = farthest depth = conservative for occlusion culling.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dst_size = textureDimensions(dst);
    if gid.x >= dst_size.x || gid.y >= dst_size.y {
        return;
    }

    let src_coord = gid.xy * 2u;
    let a = textureLoad(src, src_coord, 0).r;
    let b = textureLoad(src, src_coord + vec2<u32>(1u, 0u), 0).r;
    let c = textureLoad(src, src_coord + vec2<u32>(0u, 1u), 0).r;
    let d = textureLoad(src, src_coord + vec2<u32>(1u, 1u), 0).r;

    let max_depth = max(max(a, b), max(c, d));
    textureStore(dst, gid.xy, vec4<f32>(max_depth, 0.0, 0.0, 0.0));
}
