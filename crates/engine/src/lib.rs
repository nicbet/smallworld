//! The smallworld engine: micro-voxel world representation, streaming, and GPU
//! raymarching. See `docs/DESIGN.md` for the architecture this crate implements.
//!
//! Nothing here renders yet. This crate currently establishes the two conventions every
//! later module depends on — where WGSL shader source comes from ([`shaders`]) and where
//! runtime data is loaded from ([`assets`]) — so that a `cargo run` from any directory, a
//! test, and a shipped binary all resolve the same files on all three target platforms.

pub mod assets;
pub mod shaders;

/// Version of the engine, as declared in its `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
