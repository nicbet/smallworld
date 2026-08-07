## What

A standalone benchmark harness that evaluates `bevy_ecs` and `hecs` head-to-head on workloads representative of the smallworld engine: bulk entity insertion, tight component iteration, filtered queries, and wgpu resource ergonomics. Produces a written comparison with a recommendation for which ECS to adopt in sw-314e8f.

## Why

The engine currently uses hand-rolled `Vec<VoxelModel>` + `Vec<VoxelInstance>` in `Scene`. The OOC pipeline (E1 epic) needs per-entity state, runtime mutation, spatial queries, and lifecycle management. An ECS provides these primitives, but the wrong choice could bottleneck the render loop. This spike de-risks the decision before any production code depends on it.

## Acceptance Criteria

- [ ] Benchmark harness in `crates/bench-ecs/` (workspace member, binary crate)
- [ ] Benchmarks run at 2M+ entities with representative component archetypes
- [ ] Measures: bulk insert, iteration (dense + sparse), filtered query, add/remove component churn
- [ ] Each benchmark prints throughput (entities/sec or ns/entity)
- [ ] Written comparison saved as an xpo artifact with recommendation
- [ ] No changes to existing engine or sandbox code

## Flow

1. **Add workspace member** — create `crates/bench-ecs/` with its own `Cargo.toml` depending on `bevy_ecs`, `hecs`, `glam`, and `wgpu` (workspace dep). Add to workspace `members`.

2. **Define component archetypes** matching real engine types:
   - `Transform` — `Mat4` (64 bytes, every entity has one)
   - `WorldAabb` — two `Vec3` (24 bytes, every entity)
   - `BrickSlot` — `u32` handle (4 bytes, most entities)
   - `Visible` — zero-sized marker component for filter queries
   - `LodLevel` — `u8` (1 byte, sparse — only ~25% of entities)

3. **Implement benchmarks** (each runs N=2M entities, 10 iterations, reports median):
   - **Bulk insert** — spawn N entities with `(Transform, WorldAabb, BrickSlot)`
   - **Dense iterate** — iterate all `(Transform, WorldAabb)`, read both, accumulate checksum
   - **Sparse iterate** — iterate `(Transform, LodLevel)` where only 25% have `LodLevel`
   - **Filtered query** — iterate `(Transform, WorldAabb)` with `Visible` filter
   - **Component churn** — add then remove `LodLevel` on 10% of entities per frame, 100 frames

4. **WGPU ergonomics probe** — a qualitative section (not timed). Store a `wgpu::Buffer` handle in a component and verify:
   - Can it be stored without `unsafe`? (wgpu types are `!Send` on some backends)
   - Does the ECS require `Send + Sync` for components?
   - Resource/singleton pattern for `wgpu::Device` and `wgpu::Queue`

5. **Runner** — `main.rs` runs all benchmarks sequentially, prints a markdown table to stdout.

6. **Write comparison** — xpo artifact summarizing results, trade-offs, and recommendation.

## Decisions

- **Separate crate, not integrated** — the spike must not touch engine code. A standalone `crates/bench-ecs/` binary keeps the comparison isolated and disposable. After the decision is made, sw-314e8f will integrate the winner properly.

- **No criterion dependency** — this is a one-shot spike, not a permanent benchmark suite. Simple `Instant::now()` timing with median-of-10 is sufficient for the throughput comparison we need. Keeps the dependency footprint minimal.

- **2M entities as baseline** — the Stress preset targets ~2M bricks in a 128^3 grid. This matches real workload scale. We may also run at 5M to check scaling linearly.

- **Component sizes modeled on real types** — `Transform` as `Mat4` and `WorldAabb` as two `Vec3` mirror `VoxelInstanceGpu` fields. This ensures cache behavior in the benchmark reflects production access patterns.

- **`bevy_ecs` standalone, not full Bevy** — we only need the ECS crate. `bevy_ecs` is published independently and can be used without the rest of the Bevy engine. This avoids pulling in rendering, windowing, etc.

- **No shipyard** — sparse-set ECS trades dense iteration speed for faster component churn. The engine's hot path is iterating millions of entities per frame, making archetype-based storage (bevy_ecs, hecs) the right family. Shipyard's smaller community is also a risk for a production dependency.

- **macOS ARM only** — spike runs on local dev machine. No Linux CI targeting.

## Edge Cases

- **`!Send` wgpu types** — HIGH. On some backends (WebGL, though not relevant for us), wgpu types are `!Send`. `bevy_ecs` requires `Send + Sync` for components by default. If `wgpu::Buffer` can't be stored directly, we document the workaround (resource/newtype wrapper). This is qualitative, not a blocker — the engine uses Metal/Vulkan where wgpu types are `Send`.

- **bevy_ecs version coupling** — MEDIUM. `bevy_ecs` tracks Bevy releases and APIs shift between versions. We pin to the latest stable (0.16.x) and note API stability in the comparison.

## Assumptions

- Metal backend on macOS makes wgpu types `Send + Sync` — the `!Send` constraint only applies to web backends.
- `bevy_ecs` 0.16 is the latest stable release and is the version we evaluate.
- The benchmark doesn't need GPU execution — it's pure CPU ECS overhead measurement. The WGPU probe is qualitative only.
