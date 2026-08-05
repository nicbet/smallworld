//! Locating runtime assets.
//!
//! Runtime data — textures, material palettes, saved worlds — lives in the workspace's
//! `assets/` directory. Shaders do not: they are engine source and are baked into the
//! binary (see [`crate::shaders`]).
//!
//! Everything goes through [`root`] and [`path`] so the same tree is found whether the
//! binary was launched by `cargo run` from an arbitrary directory or shipped next to its
//! assets. Resolution order, first match wins:
//!
//! 1. `$SMALLWORLD_ASSETS` — explicit override, for packaging and tests.
//! 2. `<directory of the running executable>/assets` — the shipped layout.
//! 3. `<workspace root>/assets` — the development layout, derived from the compile-time
//!    `CARGO_MANIFEST_DIR`.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Environment variable that overrides asset-root discovery.
pub const ASSET_ROOT_ENV: &str = "SMALLWORLD_ASSETS";

/// This crate's directory at compile time, i.e. `<workspace root>/crates/engine`.
const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The directory runtime assets are loaded from.
///
/// Resolved on first call and cached for the life of the process, so a mid-run change to
/// `$SMALLWORLD_ASSETS` has no effect.
#[must_use]
pub fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let exe_dir = env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        resolve(
            env::var_os(ASSET_ROOT_ENV).map(PathBuf::from),
            exe_dir.as_deref(),
        )
    })
}

/// Resolves `relative` against the [`root`].
///
/// `relative` is always written with `/` separators regardless of platform; it is split
/// into components before joining, so the result is correct on Windows too.
#[must_use]
pub fn path(relative: &str) -> PathBuf {
    let mut resolved = root().to_path_buf();
    resolved.extend(relative.split('/').filter(|part| !part.is_empty()));
    resolved
}

/// The resolution rule itself, kept free of environment access so it is testable.
///
/// Edition 2024 makes `env::set_var` unsafe (and it races across test threads), so the
/// inputs are passed in rather than read here.
fn resolve(override_dir: Option<PathBuf>, exe_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir;
    }
    if let Some(candidate) = exe_dir.map(|dir| dir.join("assets"))
        && candidate.is_dir()
    {
        return candidate;
    }
    dev_root()
}

/// `<workspace root>/assets`, derived from this crate's compile-time manifest directory.
fn dev_root() -> PathBuf {
    let manifest_dir = Path::new(MANIFEST_DIR);
    // crates/engine -> crates -> workspace root
    manifest_dir
        .ancestors()
        .nth(2)
        .unwrap_or(manifest_dir)
        .join("assets")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<workspace root>`, the one directory these tests can rely on existing.
    fn workspace_root() -> PathBuf {
        dev_root()
            .parent()
            .expect("dev root has a parent")
            .to_path_buf()
    }

    #[test]
    fn override_wins_over_everything() {
        let forced = PathBuf::from("/somewhere/else");
        let resolved = resolve(Some(forced.clone()), Some(&workspace_root()));
        assert_eq!(resolved, forced);
    }

    #[test]
    fn exe_adjacent_assets_dir_is_used_when_present() {
        // The workspace root stands in for a shipped binary's directory: it is the one
        // place we know an `assets/` sibling exists.
        let resolved = resolve(None, Some(&workspace_root()));
        assert_eq!(resolved, dev_root());
        assert!(resolved.is_dir(), "assets/ is missing from the workspace");
    }

    #[test]
    fn falls_back_to_dev_root_without_an_exe_adjacent_dir() {
        // crates/engine has no assets/ sibling, so this exercises the fallback.
        assert_eq!(resolve(None, Some(Path::new(MANIFEST_DIR))), dev_root());
        assert_eq!(resolve(None, None), dev_root());
    }

    #[test]
    fn dev_root_points_at_the_workspace() {
        assert!(workspace_root().join("Cargo.toml").is_file());
        assert!(dev_root().ends_with("assets"));
    }

    #[test]
    fn relative_paths_join_component_wise() {
        let joined = path("textures/stone/albedo.png");
        assert!(joined.starts_with(root()));
        assert!(joined.ends_with(Path::new("textures").join("stone").join("albedo.png")));
    }

    #[test]
    fn empty_and_untidy_relative_paths_are_tolerated() {
        assert_eq!(path(""), root());
        assert_eq!(path("worlds//alpha"), path("worlds/alpha"));
    }
}
