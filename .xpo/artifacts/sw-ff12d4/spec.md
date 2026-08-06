# Sandbox scaffold: move viewer/worldgen/model_gen into crates/sandbox

## What

Rename `crates/viewer` to `crates/sandbox` and relocate `worldgen.rs` and `model_gen.rs` from the engine crate into the sandbox crate. The engine retains only core systems; test-scene content lives in the sandbox.

## Why

The engine is a library — it provides rendering, data structures, and GPU abstractions. World generation and procedural model generation are test content, not engine concerns. Moving them out keeps the engine's public surface clean and establishes the sandbox as the integration testbed from day zero.

## Flow

1. **Create `crates/sandbox/`** by renaming `crates/viewer/` (git mv)
2. **Move source files** from engine to sandbox:
   - `crates/engine/src/worldgen.rs` → `crates/sandbox/src/worldgen.rs`
   - `crates/engine/src/model_gen.rs` → `crates/sandbox/src/model_gen.rs`
3. **Update `crates/sandbox/Cargo.toml`**:
   - Package name: `smallworld-sandbox`
   - Binary name stays `smallworld`
   - Description updated to reflect sandbox role
4. **Update moved modules** — change `crate::` imports to `smallworld_engine::`:
   - `worldgen.rs`: `crate::brick_pool::{BRICK_EDGE, BRICK_VOLUME, VOXEL_SCALE}` → `smallworld_engine::brick_pool::{BRICK_EDGE, BRICK_VOLUME, VOXEL_SCALE}`
   - `model_gen.rs`: `crate::brick_pool::{BrickPool, BRICK_EDGE, BRICK_VOLUME}` → `smallworld_engine::brick_pool::{BrickPool, BRICK_EDGE, BRICK_VOLUME}`, `crate::voxel_object::VoxelModel` → `smallworld_engine::voxel_object::VoxelModel`
5. **Update `crates/sandbox/src/main.rs`**:
   - Replace `smallworld_engine::model_gen` with `mod model_gen`
   - Replace `smallworld_engine::worldgen::WorldGenerator` / `hash_for_placement` with `mod worldgen` local imports
   - All other engine imports unchanged
6. **Update `crates/engine/src/lib.rs`**:
   - Remove `pub mod model_gen;` and `pub mod worldgen;`
7. **Update workspace `Cargo.toml`**:
   - Members: replace `crates/viewer` with `crates/sandbox`
   - Add `[profile.dev.package.smallworld-sandbox] opt-level = 3` (worldgen noise was the reason for engine's opt-level override; it now lives in sandbox)
8. **Update `Makefile`**:
   - `VIEWER := smallworld-viewer` → `SANDBOX := smallworld-sandbox`
   - All `-p $(VIEWER)` → `-p $(SANDBOX)`
   - Rename `run` target help text to "Run the sandbox"

## Decisions

- **Binary name stays `smallworld`** — the user types `cargo run` or `make run`, the binary name is irrelevant to the workflow and avoids churn in CI/screenshots.
- **Keep engine opt-level = 3** — engine internals (brick_pool, raymarcher bindings) still benefit from optimization in dev builds.
- **Add sandbox opt-level = 3** — worldgen noise computations that motivated the original override now live here.
- **No re-export from engine** — worldgen/model_gen are sandbox-private modules, not `pub`. The engine's public API loses two modules it never should have exposed.

## Acceptance Criteria

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes (engine BVH tests still run; worldgen/model_gen have no tests to migrate)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all --check` clean
- [ ] `make run` launches the sandbox with the same terrain + objects scene as before
- [ ] `make smoke` prints adapter info and exits 0
- [ ] `crates/engine/src/` contains no `worldgen.rs` or `model_gen.rs`
- [ ] `crates/viewer/` directory no longer exists
- [ ] Engine `lib.rs` does not export `worldgen` or `model_gen`
