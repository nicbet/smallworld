//! WGSL shader sources.
//!
//! Shaders are engine source, not user-facing assets: each one lives in
//! `crates/engine/shaders/` and is baked into the binary with [`include_str!`], so a
//! shipped viewer can never fail to find one.
//!
//! Call sites use [`load`] rather than `include_str!` directly. The indirection buys a
//! development override: with `$SMALLWORLD_SHADER_DIR` set, [`load`] reads the file from
//! that directory instead, so shader iteration does not require a rebuild. With the
//! variable unset — the only case in a shipped build — the baked source is returned
//! without touching the filesystem.
//!
//! WGSL has no `#include`. Shared declarations live in [`Shader::Common`] and are
//! prepended on the Rust side with [`compose`].

use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::Path;

/// Environment variable pointing at a directory of WGSL files to load instead of the
/// baked ones. Files missing from it fall back to the baked source individually.
pub const SHADER_DIR_ENV: &str = "SMALLWORLD_SHADER_DIR";

/// A WGSL source file shipped with the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Shader {
    /// Constants and helpers shared by every pass; prepend it with [`compose`].
    Common,
}

impl Shader {
    /// This shader's file name within `crates/engine/shaders/`.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Common => "common.wgsl",
        }
    }

    /// The source baked in at compile time, ignoring any [`SHADER_DIR_ENV`] override.
    #[must_use]
    pub const fn baked(self) -> &'static str {
        match self {
            Self::Common => include_str!("../shaders/common.wgsl"),
        }
    }
}

/// Returns the WGSL source for `shader`.
///
/// Reads `$SMALLWORLD_SHADER_DIR/<file name>` when that variable is set and the file is
/// readable; otherwise returns the baked source. An unreadable override is reported on
/// stderr and does not fail the call — a typo in the variable should not take the
/// renderer down.
#[must_use]
pub fn load(shader: Shader) -> Cow<'static, str> {
    let overridden = env::var_os(SHADER_DIR_ENV).and_then(|dir| read_from(Path::new(&dir), shader));
    match overridden {
        Some(source) => Cow::Owned(source),
        None => Cow::Borrowed(shader.baked()),
    }
}

/// Concatenates the given shaders into a single WGSL module, in order.
///
/// Each source is preceded by a comment naming its file, which is what makes a naga
/// error line number traceable back to the file it came from.
#[must_use]
pub fn compose(shaders: &[Shader]) -> String {
    let mut composed = String::new();
    for shader in shaders {
        composed.push_str("// ---- ");
        composed.push_str(shader.file_name());
        composed.push_str(" ----\n");
        composed.push_str(&load(*shader));
        if !composed.ends_with('\n') {
            composed.push('\n');
        }
    }
    composed
}

/// Reads one shader out of an override directory, or `None` if it cannot be read.
fn read_from(dir: &Path, shader: Shader) -> Option<String> {
    let path = dir.join(shader.file_name());
    match fs::read_to_string(&path) {
        Ok(source) => Some(source),
        Err(err) => {
            eprintln!(
                "{SHADER_DIR_ENV}: cannot read {} ({err}); using the baked source",
                path.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Every shader the engine knows about. Extend alongside [`Shader`].
    const ALL: &[Shader] = &[Shader::Common];

    fn shader_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("shaders")
    }

    #[test]
    fn baked_sources_are_non_empty() {
        for shader in ALL {
            assert!(!shader.baked().trim().is_empty(), "{shader:?} is empty");
        }
    }

    #[test]
    fn file_names_match_the_baked_files() {
        // Catches a `file_name` that has drifted from its `include_str!` path: the
        // override would silently read a different file than the one compiled in.
        for shader in ALL {
            let from_disk = read_from(&shader_dir(), *shader);
            assert_eq!(
                from_disk.as_deref(),
                Some(shader.baked()),
                "{shader:?}: {} does not match the baked source",
                shader.file_name()
            );
        }
    }

    #[test]
    fn missing_override_directory_falls_back_to_baked() {
        for shader in ALL {
            assert_eq!(read_from(Path::new("/no/such/shader/dir"), *shader), None);
        }
    }

    #[test]
    fn compose_concatenates_in_order_with_file_markers() {
        let composed = compose(&[Shader::Common, Shader::Common]);
        assert_eq!(composed.matches("// ---- common.wgsl ----").count(), 2);
        assert!(composed.contains(Shader::Common.baked().trim()));
        assert!(composed.ends_with('\n'));
    }

    #[test]
    fn compose_of_nothing_is_empty() {
        assert!(compose(&[]).is_empty());
    }
}
