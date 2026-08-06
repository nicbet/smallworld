//! Trait for providing brick data to the pager.

use crate::brick_data::BrickData;
use glam::Vec3;

/// Games implement this to feed brick data into the [`BrickPager`](crate::brick_pager::BrickPager).
///
/// `generate()` is called on background worker threads — implementations must be
/// `Send + Sync` and must not access the GPU. Return `None` for empty (air) cells.
pub trait BrickSource: Send + Sync {
    /// Produce brick data for the given grid cell.
    ///
    /// `grid_pos` is the integer grid coordinate. `world_min` is the world-space
    /// origin of the grid, so the brick's world-space corner is
    /// `world_min + grid_pos.as_vec3() * brick_size`.
    fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData>;
}
