## What was built

A standalone benchmark crate (`crates/bench-ecs/`) comparing three approaches for entity storage at 2M entities: plain `Vec` (parallel arrays), `hecs` 0.11, and `bevy_ecs` 0.19. Five benchmarks (bulk insert, dense iterate, sparse iterate, filtered query, component churn) plus a qualitative WGPU integration probe.

## Why

The OOC pipeline needs per-entity state, runtime mutation, and subset queries. Before committing to an ECS library, we needed to know (a) which ECS is faster and (b) what overhead ECS adds over hand-rolled arrays. The spike de-risked both questions before any production code depends on the choice.

## How the pieces fit together

```
crates/bench-ecs/
├── Cargo.toml          — depends on bevy_ecs, hecs, glam, wgpu, pollster
└── src/
    ├── main.rs          — harness infra (run_bench, median, print_table), runs all three
    ├── vec_bench.rs     — parallel-array baseline using Vec + Option/bool for sparse data
    ├── bevy_bench.rs    — bevy_ecs benchmarks with #[derive(Component)] types
    ├── hecs_bench.rs    — hecs benchmarks with plain structs
    └── wgpu_probe.rs    — qualitative test: store wgpu::Buffer as component/resource
```

Each benchmark module defines its own component types (bevy_ecs requires `#[derive(Component)]`, hecs doesn't — shared types with cfg-gating was tried and rejected). All modules use the same `make_transform(i)` / `make_aabb(i)` helpers from `main.rs` for consistency.

The Vec baseline uses parallel arrays (`Vec<Transform>`, `Vec<WorldAabb>`, `Vec<Option<LodLevel>>`, `Vec<bool>`) — the simplest possible approach, representing what the engine does today with `Scene`.

## Key decisions

- **Separate crate** — no changes to engine or sandbox code. The spike is disposable after the decision is made.
- **No criterion** — `Instant::now()` with median-of-10. One-shot spike, not a permanent benchmark suite.
- **Entity IDs collected during spawn** — hecs 0.11 changed its query iterator API (yields `Q::Item` directly, no entity). Collecting IDs at spawn avoids API differences in the benchmark hot path.
- **Component sizes match production types** — `Mat4` (64B) for transforms, `Vec3` pair (24B) for AABBs. Cache behavior in benchmarks reflects real access patterns.

## Key findings

The Vec baseline revealed the true cost of ECS:

- **ECS wins on subset queries** — 4-7× faster than scanning `Vec<Option>` or `Vec<bool>`. This is the value proposition.
- **ECS loses on insert** (~5×) and **dense iteration** (~36%). Acceptable overhead.
- **Component churn is catastrophic** — 500-660× slower than field mutation. Archetype migration moves all entity data between tables. This shaped the component design rules now in CLAUDE.md.

hecs beat bevy_ecs in 4/5 benchmarks. bevy_ecs won sparse iteration (archetype graph) but its always-on change detection and heavier metadata cost it everywhere else.

## Outcome

**Decision: adopt hecs 0.11.** Component design rules codified in CLAUDE.md to prevent the churn antipattern. Full benchmark data and analysis in the `ecs-comparison.md` artifact.
