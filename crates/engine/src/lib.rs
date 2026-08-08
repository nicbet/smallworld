//! The smallworld engine: micro-voxel world representation, streaming, and GPU
//! raymarching. See `docs/DESIGN.md` for the architecture this crate implements.

pub mod assets;
pub mod brick_data;
pub mod brick_index;
pub mod brick_pager;
pub mod brick_pool;
pub mod brick_source;
pub mod bvh;
pub mod camera;
pub mod cull;
pub mod engine;
pub mod gpu;
pub mod light;
pub mod material;
pub mod mesh;
pub mod gpu_timing;
pub mod input;
pub(crate) mod jobs;
pub mod gbuffer;
pub mod lighting;
pub mod raymarcher;
pub mod shaders;
pub mod stream;
pub mod svo;
pub mod volume;
pub mod voxel_object;
pub mod world;

/// Re-export wgpu so the viewer (and future crates) share one version.
pub use wgpu;

/// Version of the engine, as declared in its `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
