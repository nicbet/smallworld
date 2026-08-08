//! Texture image data stored in the [`World`](crate::world::World).
//!
//! Textures hold CPU-side pixel data. The rendering pipeline uploads
//! them to the GPU on first use and caches the GPU handles internally.

/// CPU-side texture image. Always stored as RGBA8 sRGB.
pub struct TextureData {
    /// Raw pixel bytes (RGBA, 4 bytes per pixel).
    pub pixels: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}
