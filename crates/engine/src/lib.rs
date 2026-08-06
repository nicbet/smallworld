//! The smallworld engine: micro-voxel world representation, streaming, and GPU
//! raymarching. See `docs/DESIGN.md` for the architecture this crate implements.

pub mod assets;
pub mod brick_index;
pub mod brick_pool;
pub mod bvh;
pub mod camera;
pub mod gpu;
pub mod gpu_timing;
pub mod model_gen;
pub mod raymarcher;
pub mod scene;
pub mod shaders;
pub mod voxel_object;
pub mod worldgen;

/// Re-export wgpu so the viewer (and future crates) share one version.
pub use wgpu;

/// Version of the engine, as declared in its `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
