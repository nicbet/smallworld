//! Shared data types for brick content passed between sources and the pager.

/// Raw brick content: voxels and palette, ready for GPU upload.
pub struct BrickData {
    /// 16³ = 4096 palette indices (0 = air).
    pub voxels: [u8; 4096],
    /// RGBA palette entries. Up to 256.
    pub palette: Vec<[u8; 4]>,
}
