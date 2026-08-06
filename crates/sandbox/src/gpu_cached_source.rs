//! [`BrickSource`] that pulls from the GPU results cache, with region + CPU fallbacks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use glam::Vec3;
use smallworld_engine::brick_data::BrickData;
use smallworld_engine::brick_source::BrickSource;

use crate::cached_source::CachedSource;
use crate::worldgen::WorldGenerator;

/// `BrickSource` backed by GPU-generated results.
///
/// Lookup order: GPU cache → region disk cache → CPU fallback.
pub struct GpuCachedSource {
    gpu_cache: Arc<Mutex<HashMap<[u32; 3], Option<BrickData>>>>,
    disk_source: CachedSource<WorldGenerator>,
}

impl GpuCachedSource {
    /// Creates a source that reads from the GPU cache first, then falls back
    /// to `CachedSource<WorldGenerator>` (disk → CPU noise).
    pub fn new(
        gpu_cache: Arc<Mutex<HashMap<[u32; 3], Option<BrickData>>>>,
        cpu_gen: WorldGenerator,
        region_dir: PathBuf,
    ) -> Self {
        Self {
            gpu_cache,
            disk_source: CachedSource::new(cpu_gen, region_dir),
        }
    }
}

impl BrickSource for GpuCachedSource {
    fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
        if let Some(entry) = self.gpu_cache.lock().unwrap().remove(&grid_pos) {
            // Persist GPU results: the GPU and CPU generators are not
            // bit-identical (sw-c9d281), so the first-generated
            // content must win durably or the world changes between runs.
            self.disk_source.store(grid_pos, entry.as_ref());
            return entry;
        }
        self.disk_source.generate(grid_pos, world_min)
    }
}
