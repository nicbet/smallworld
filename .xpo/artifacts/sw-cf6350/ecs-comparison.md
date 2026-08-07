# ECS A/B Spike: bevy_ecs 0.19 vs hecs 0.11 vs Vec baseline

## Environment

- **Platform:** macOS ARM (Apple Silicon)
- **Rust:** 1.97, `--release` profile
- **Entity count:** 2,000,000
- **Iterations:** 10 (median reported)

## Component Archetypes

Sized to match real engine types:

| Component | Size | Distribution |
|-----------|------|-------------|
| Transform | `Mat4` (64 B) | every entity |
| WorldAabb | 2× `Vec3` (24 B) | every entity |
| BrickSlot | `u32` (4 B) | every entity |
| Visible | ZST marker / bool | 50% of entities |
| LodLevel | `u8` (1 B) | 25% of entities |

## Results

### Throughput (all three)

| Benchmark | Vec baseline | hecs | bevy_ecs |
|-----------|-------------|------|----------|
| bulk_insert | **27 ms** | 125 ms (4.6×) | 145 ms (5.3×) |
| dense_iterate | **2.83 ms** | 3.86 ms (1.36×) | 3.91 ms (1.38×) |
| sparse_iterate | 3.70 ms | 707 µs | **552 µs** |
| filtered_query | 7.06 ms | **1.89 ms** | 1.92 ms |
| component_churn | **3.81 ms** | 2.03 s (533×) | 2.51 s (659×) |

### Analysis

**Where ECS wins (and why it's worth adopting):**

- **Sparse iteration** — ECS is 5-7× faster than Vec. Archetype storage means querying "all entities with LodLevel" touches only the 500K that have it, instead of scanning 2M `Option`s. This advantage grows with entity count and component diversity.
- **Filtered queries** — ECS is 3.7× faster. Same reason: archetype tables skip entities that don't match, while Vec must branch on every element.

**Where ECS loses (and how to avoid the penalty):**

- **Bulk insert** — ECS is 4.6-5.3× slower. Archetype bookkeeping has real per-entity cost. Mitigation: batch spawns at load time, not per-frame. This is already our pattern (BrickPager preloads).
- **Dense iteration** — ECS is ~36% slower. Acceptable — the absolute cost is still sub-4ms for 2M entities. The hot loop is membound either way.
- **Component churn** — ECS is **500-660× slower**. This is the critical finding. Archetype migration moves all entity data between tables on every add/remove. Vec just writes an `Option`.

### The Churn Rule

This is the most important takeaway from the spike:

> **Never model frequently-changing state as component presence/absence.**
> Use a mutable field inside a component instead.

| Pattern | When to use | Example |
|---------|-------------|---------|
| Component add/remove | Structural identity, set at spawn | `TerrainChunk`, `Prop`, `Particle` |
| Mutable field in component | State that changes at runtime | `LodLevel(u8)`, `Visible(bool)`, `StreamPriority(f32)` |
| Component add/remove cost | ~10 µs per 200K ops | — |
| Field mutation cost | ~19 ns per 200K ops | — |

## ECS vs ECS

### hecs vs bevy_ecs head-to-head

| Benchmark | bevy_ecs | hecs | Winner |
|-----------|----------|------|--------|
| bulk_insert | 145 ms | **125 ms** | hecs (1.16×) |
| dense_iterate | 3.91 ms | **3.86 ms** | hecs (1.01×) |
| sparse_iterate | **552 µs** | 707 µs | bevy_ecs (1.28×) |
| filtered_query | 1.92 ms | **1.89 ms** | hecs (1.02×) |
| component_churn | 2.51 s | **2.03 s** | hecs (1.24×) |

hecs wins 4 of 5. bevy_ecs wins sparse iteration thanks to its archetype graph but pays for it with heavier metadata overhead everywhere else.

## WGPU Integration

| Capability | bevy_ecs | hecs |
|-----------|----------|------|
| `wgpu::Buffer` as component | ✓ (Send+Sync on Metal) | ✓ (Send+Sync+'static) |
| `wgpu::Device` as singleton | ✓ via `Resource` derive | ✗ no built-in resource system |
| `wgpu::Queue` as singleton | ✓ via `Resource` derive | ✗ store outside World |
| Non-Send resource support | ✓ `insert_non_send_resource` | ✗ N/A |

bevy_ecs has a first-class resource/singleton system. hecs has none — GPU objects live in a separate struct alongside the World. This is fine for the engine since `GpuContext` is already managed separately.

## Trade-offs Beyond Benchmarks

### bevy_ecs advantages
- **Resource system** — Device, Queue, pipeline objects are first-class World citizens
- **Change detection** — `Changed<T>`, `Added<T>` filters avoid polling
- **System scheduling** — built-in parallel system executor
- **Ecosystem** — Bevy plugins, community crates, extensive docs
- **Query filters** — `With<T>`, `Without<T>`, `Or<T>`, `Changed<T>` built in

### hecs advantages
- **Raw throughput** — faster in 4/5 benchmarks
- **Minimal API surface** — ~2K LOC, trivial to audit
- **No framework opinions** — no scheduler, no stages, no plugins
- **Compile time** — one crate vs 10+ transitive dependencies
- **Stability** — tiny API surface, fewer breaking changes

### bevy_ecs risks
- **Version coupling** — tracks Bevy releases, major API churn every ~4 months
- **Feature creep** — pulls in reflection, tasks, utils, ptr — 5 extra crates
- **Change detection overhead** — always-on, can't opt out per-archetype

### hecs risks
- **No change detection** — dirty-flagging must be manual (bitset or generation counter)
- **No resource system** — GPU objects managed separately
- **Smaller community** — fewer eyes on bugs

## Recommendation: hecs

For the smallworld engine, **hecs** is the right choice.

1. **Performance where it matters.** Dense iteration and insert are the hot paths. hecs wins both and is closer to the Vec baseline.
2. **We don't need a framework.** The engine has its own render loop, GPU context, and frame scheduling.
3. **Minimal surface area.** ~2K LOC, stable API, one crate.
4. **Change detection is cheap to add.** Generation counters or bitsets cover GPU upload dirty-flagging.
5. **Compile time.** One crate vs 10+ transitive deps.

## Component Design Rules (for all future implementation)

1. **Structural identity → component.** `TerrainChunk`, `VoxelProp`, `PointLight` — set at spawn, rarely or never removed.
2. **Runtime state → mutable field.** `LodLevel(u8)`, `Visible(bool)`, `StreamingState(enum)` — changes per-frame, must not trigger archetype migration.
3. **GPU handles → side struct, not ECS.** `Device`, `Queue`, pipelines, and buffer pools live in `GpuContext`, not as components or resources.
4. **Batch spawns at load time.** ECS insert is 4.6× slower than Vec push. Spawn during preload/streaming, never in the render loop.
5. **Query for subsets, iterate for bulk.** Use ECS queries when you need "all entities with X" (culling, LOD selection). For "every entity" passes, the 36% overhead is the cost of flexibility.
