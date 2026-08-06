//! Disk-backed [`BrickSource`] wrapper: region cache with generate-on-miss.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use glam::Vec3;
use smallworld_engine::brick_data::BrickData;
use smallworld_engine::brick_source::BrickSource;

use crate::region::{self, RegionFile};

/// Wraps a [`BrickSource`] with on-disk region file caching.
///
/// First call for a cell checks the region file. On miss, delegates to the
/// inner source, writes the result back, and returns it. Subsequent calls
/// (even across process restarts) hit the cache.
pub struct CachedSource<S> {
    inner: S,
    region_dir: PathBuf,
    regions: RwLock<HashMap<[u32; 3], Arc<Mutex<RegionFile>>>>,
}

impl<S: BrickSource> CachedSource<S> {
    /// Creates a cached source writing region files under `region_dir`.
    pub fn new(inner: S, region_dir: PathBuf) -> Self {
        Self {
            inner,
            region_dir,
            regions: RwLock::new(HashMap::new()),
        }
    }

    fn get_or_open_region(&self, region_pos: [u32; 3]) -> Arc<Mutex<RegionFile>> {
        {
            let map = self.regions.read().unwrap();
            if let Some(region) = map.get(&region_pos) {
                return Arc::clone(region);
            }
        }

        let mut map = self.regions.write().unwrap();
        Arc::clone(map.entry(region_pos).or_insert_with(|| {
            let path = region::region_path(&self.region_dir, region_pos);
            Arc::new(Mutex::new(
                RegionFile::open_or_create(&path).expect("failed to open region file"),
            ))
        }))
    }
}

impl<S: BrickSource> BrickSource for CachedSource<S> {
    fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
        let (region_pos, local_pos) = region::split_grid_pos(grid_pos);
        let region_arc = self.get_or_open_region(region_pos);
        let mut region = region_arc.lock().unwrap();

        if region.has_entry(local_pos) {
            return match region.read_brick(local_pos) {
                Ok(data) => data,
                Err(e) => {
                    log::warn!("region read error at {grid_pos:?}: {e}");
                    self.inner.generate(grid_pos, world_min)
                }
            };
        }

        let data = self.inner.generate(grid_pos, world_min);

        if let Err(e) = region.write_brick(local_pos, data.as_ref()) {
            log::warn!("region write error at {grid_pos:?}: {e}");
        }

        data
    }
}

/// Returns the default cache directory for a preset.
pub fn cache_dir_for_preset(preset_name: &str) -> PathBuf {
    let base = if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
        dirs_cache_base()
    } else {
        dirs_local_app_data()
    };
    base.join("smallworld")
        .join("sandbox")
        .join(preset_name)
        .join("regions")
}

fn dirs_cache_base() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join(".cache")
}

fn dirs_local_app_data() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
