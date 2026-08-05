//! Debug viewer and application shell for the smallworld engine.
//!
//! Until the window and wgpu device land, `main` is a smoke test: it reports the build
//! and the paths the engine resolved, which is exactly what the CI matrix needs to see
//! working on macOS, Windows and Linux before any GPU code is written.

use std::env;

use smallworld_engine::{VERSION, assets, shaders};

fn main() {
    let common = shaders::load(shaders::Shader::Common);

    println!("smallworld-viewer {}", env!("CARGO_PKG_VERSION"));
    println!("  engine      {VERSION}");
    println!("  host        {}/{}", env::consts::OS, env::consts::ARCH);
    println!("  assets      {}", assets::root().display());
    println!(
        "  shaders     {} ({} bytes)",
        shaders::Shader::Common.file_name(),
        common.len()
    );
}
