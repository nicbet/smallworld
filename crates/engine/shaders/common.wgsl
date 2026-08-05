// Constants and helpers shared by every smallworld pass.
//
// WGSL has no #include: this file is prepended to other shader sources on the Rust side
// (see `shaders::compose`). Keep it free of entry points, bindings and global state —
// anything declared here is declared in every module that composes it.

// Voxels along one brick edge (docs/DESIGN.md §3).
const BRICK_EDGE: u32 = 16u;

// Voxels in one brick.
const BRICK_VOLUME: u32 = BRICK_EDGE * BRICK_EDGE * BRICK_EDGE;

// Edge length of one voxel in metres at base scale (docs/DESIGN.md D3).
const VOXEL_SCALE: f32 = 0.1;

// Flattens a voxel coordinate within a brick to its index in the brick payload.
// x is the fastest-varying axis, matching the CPU-side layout.
fn brick_index(voxel: vec3<u32>) -> u32 {
    return voxel.x + BRICK_EDGE * (voxel.y + BRICK_EDGE * voxel.z);
}
