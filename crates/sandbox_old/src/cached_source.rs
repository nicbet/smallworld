//! Disk-backed [`BrickSource`] wrapper: region cache with generate-on-miss.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use glam::Vec3;
use smallworld_engine::brick_data::BrickData;
use smallworld_engine::brick_source::BrickSource;

use crate::region::{self, RegionFile};

/// Bump when anything that changes generated brick content changes (worldgen
/// logic, palette, region format). A mismatch wipes the preset's region
/// cache — stale entries from older generators must never mix with fresh
/// results, or the cached world diverges from the generated one.
pub const CACHE_VERSION: u32 = 2;

/// Maximum region files held open at once. Regions are 16³ cells, so a
/// 1 km world has 4096 of them — far beyond the default macOS fd limit
/// (256). Evicted entries close when their last in-flight reference drops.
const MAX_OPEN_REGIONS: usize = 64;

/// Wraps a [`BrickSource`] with on-disk region file caching.
///
/// First call for a cell checks the region file. On miss, delegates to the
/// inner source, writes the result back, and returns it. Subsequent calls
/// (even across process restarts) hit the cache.
///
/// Cache I/O is strictly best-effort: any failure (fd exhaustion, bad file,
/// full disk) degrades to the inner source. A cache must never take down a
/// worker thread.
pub struct CachedSource<S> {
    inner: S,
    region_dir: PathBuf,
    regions: RwLock<HashMap<[u32; 3], Arc<Mutex<RegionFile>>>>,
    /// Region keys in rough open order, for eviction once over the cap.
    open_order: Mutex<Vec<[u32; 3]>>,
}

impl<S: BrickSource> CachedSource<S> {
    /// Creates a cached source writing region files under `region_dir`.
    /// Wipes the directory if its `VERSION` stamp does not match
    /// [`CACHE_VERSION`].
    pub fn new(inner: S, region_dir: PathBuf) -> Self {
        ensure_cache_version(&region_dir);
        Self {
            inner,
            region_dir,
            regions: RwLock::new(HashMap::new()),
            open_order: Mutex::new(Vec::new()),
        }
    }

    /// Opens (or returns the already-open) region file, evicting the oldest
    /// open region beyond [`MAX_OPEN_REGIONS`]. Returns `None` if the file
    /// cannot be opened — callers fall back to the inner source.
    fn get_or_open_region(&self, region_pos: [u32; 3]) -> Option<Arc<Mutex<RegionFile>>> {
        {
            let map = self.regions.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(region) = map.get(&region_pos) {
                return Some(Arc::clone(region));
            }
        }

        let path = region::region_path(&self.region_dir, region_pos);
        let file = match RegionFile::open_or_create(&path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("region open failed at {region_pos:?}: {e}; bypassing cache");
                return None;
            }
        };

        let mut map = self.regions.write().unwrap_or_else(PoisonError::into_inner);
        // Raced another thread opening the same region: use theirs.
        if let Some(region) = map.get(&region_pos) {
            return Some(Arc::clone(region));
        }

        let mut order = self
            .open_order
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while map.len() >= MAX_OPEN_REGIONS {
            let Some(oldest) = order.first().copied() else {
                break;
            };
            order.remove(0);
            map.remove(&oldest);
        }
        order.push(region_pos);

        let arc = Arc::new(Mutex::new(file));
        map.insert(region_pos, Arc::clone(&arc));
        Some(arc)
    }

    /// Persists an externally-generated result (e.g. a GPU-primed brick)
    /// into the region cache without touching the inner source. First write
    /// wins — later runs replay the same world even if generator backends
    /// disagree at the bit level.
    pub fn store(&self, grid_pos: [u32; 3], data: Option<&BrickData>) {
        let (region_pos, local_pos) = region::split_grid_pos(grid_pos);
        let Some(region_arc) = self.get_or_open_region(region_pos) else {
            return;
        };
        let mut region = region_arc.lock().unwrap_or_else(PoisonError::into_inner);
        if !region.has_entry(local_pos)
            && let Err(e) = region.write_brick(local_pos, data)
        {
            log::warn!("region write error at {grid_pos:?}: {e}");
        }
    }

    /// Number of region files currently held open (test/diagnostics).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open_region_count(&self) -> usize {
        self.regions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

impl<S: BrickSource> BrickSource for CachedSource<S> {
    fn generate(&self, grid_pos: [u32; 3], world_min: Vec3) -> Option<BrickData> {
        let (region_pos, local_pos) = region::split_grid_pos(grid_pos);
        let Some(region_arc) = self.get_or_open_region(region_pos) else {
            return self.inner.generate(grid_pos, world_min);
        };
        let mut region = region_arc.lock().unwrap_or_else(PoisonError::into_inner);

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

/// Wipes `region_dir` unless its `VERSION` stamp matches [`CACHE_VERSION`],
/// then stamps it.
fn ensure_cache_version(region_dir: &PathBuf) {
    let stamp = region_dir.join("VERSION");
    let current = fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if current == Some(CACHE_VERSION) {
        return;
    }
    if region_dir.exists() {
        log::info!(
            "region cache at {} is version {current:?}, want {CACHE_VERSION} — wiping",
            region_dir.display()
        );
        if let Err(e) = fs::remove_dir_all(region_dir) {
            log::warn!("failed to wipe stale region cache: {e}");
        }
    }
    if let Err(e) =
        fs::create_dir_all(region_dir).and_then(|()| fs::write(&stamp, CACHE_VERSION.to_string()))
    {
        log::warn!("failed to stamp region cache version: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    struct NullSource;
    impl BrickSource for NullSource {
        fn generate(&self, _grid_pos: [u32; 3], _world_min: Vec3) -> Option<BrickData> {
            None
        }
    }

    /// Touching more regions than the cap must not accumulate open files —
    /// the original leak held all 4096 Large World regions open and blew
    /// the process fd limit, killing pager workers (sw-d8e0d5).
    #[test]
    fn open_regions_stay_capped() {
        let dir = std::env::temp_dir().join("smallworld_test_region_cap");
        let _ = fs::remove_dir_all(&dir);
        let source = CachedSource::new(NullSource, dir.clone());

        for r in 0..(MAX_OPEN_REGIONS as u32 * 3) {
            // One cell per distinct region (regions are 16³ cells).
            let _ = source.generate([r * 16, 0, 0], Vec3::ZERO);
            assert!(
                source.open_region_count() <= MAX_OPEN_REGIONS,
                "open regions exceeded cap at region {r}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A version mismatch must wipe the cache; a match must keep it.
    #[test]
    fn version_mismatch_wipes_cache() {
        let dir = std::env::temp_dir().join("smallworld_test_region_version");
        let _ = fs::remove_dir_all(&dir);

        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("VERSION"), "1").unwrap();
        let marker = dir.join("r.0.0.0.swr");
        fs::write(&marker, b"stale").unwrap();

        let _source = CachedSource::new(NullSource, dir.clone());
        assert!(!marker.exists(), "stale region file must be wiped");
        assert_eq!(
            fs::read_to_string(dir.join("VERSION")).unwrap().trim(),
            CACHE_VERSION.to_string()
        );

        // Same version: contents survive.
        let marker2 = dir.join("r.0.0.1.swr");
        fs::write(&marker2, b"fresh").unwrap();
        let _source2 = CachedSource::new(NullSource, dir.clone());
        assert!(marker2.exists(), "matching-version cache must be kept");

        let _ = fs::remove_dir_all(&dir);
    }
}
