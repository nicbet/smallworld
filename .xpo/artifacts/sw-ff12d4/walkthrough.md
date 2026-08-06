# Walkthrough: Sandbox scaffold

## What changed

The `crates/viewer` binary crate was renamed to `crates/sandbox`, and two modules — `worldgen.rs` and `model_gen.rs` — were moved out of the engine library crate into the sandbox.

## Why

The engine is a library. World generation (terrain noise, density sampling) and procedural model generation (trees, rocks, pebbles) are test content used to exercise the engine during development. Keeping them in the engine polluted its public API with things no downstream consumer would use. The sandbox is the right home: it's the developer tool that exercises engine features in isolation.

## How the pieces fit

**Before:**
```
crates/engine/src/lib.rs  →  pub mod worldgen, pub mod model_gen
crates/viewer/src/main.rs →  use smallworld_engine::{worldgen, model_gen}
```

**After:**
```
crates/engine/src/lib.rs      →  no worldgen or model_gen exports
crates/sandbox/src/main.rs    →  mod worldgen; mod model_gen;  (local modules)
crates/sandbox/src/worldgen.rs →  uses smallworld_engine::brick_pool::{BRICK_EDGE, BRICK_VOLUME, VOXEL_SCALE}
crates/sandbox/src/model_gen.rs → uses smallworld_engine::brick_pool::{BrickPool, ...} + ::voxel_object::VoxelModel + ::wgpu
```

The moved modules consume the engine's public API (`BrickPool`, `VoxelModel`, `BRICK_EDGE`, etc.) exactly the same way any future game crate would. This validates the engine's integration surface from day zero.

## Key decisions

- **Binary name stays `smallworld`** — no churn in CI, screenshots, or muscle memory.
- **Added `[profile.dev.package.smallworld-sandbox] opt-level = 3`** — the worldgen noise functions that motivated the original engine opt-level override now live here. Without this, debug-mode worldgen would regress to ~10× slower.
- **Makefile target renamed `run` → `sandbox`** (done by the user during review) — clearer intent now that it's not a "viewer."
- **`cargo fmt` reformatted several engine files** — edition 2024 rustfmt rules hadn't been applied to those files yet. The diff is cosmetic (line wrapping, import order) but lands in this commit since `fmt --check` is a CI gate.

## Non-obvious details

- `model_gen.rs` needed an explicit `use smallworld_engine::wgpu;` — it references `wgpu::Queue` in function signatures, which previously resolved via `crate::` when it lived inside the engine (which has `wgpu` as a direct dependency). The engine's `pub use wgpu;` re-export makes this work without adding `wgpu` to the sandbox's `Cargo.toml`.
- The 4 worldgen unit tests now run under `target/debug/deps/smallworld-*` (the sandbox binary's test harness) instead of the engine's. `cargo test --workspace` still finds and runs them.
