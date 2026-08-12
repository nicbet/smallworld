## Profiling & Instrumentation

_(OQ 24 resolution, 2026-08-11.)_ One instrumentation API, multiple sinks — the shape UE (`stat` + Unreal Insights) and Unity (`ProfilerMarker` + Profiler) both converged on, with **Tracy playing the deep-timeline role** so we never build one. Engine/game split: the engine owns the markers API, collection across all execution contexts, and the sinks; games annotate their own systems and behaviors through the _same_ macros and read the same overlay.

### The Instrumentation API (engine primitive)

```rust
profile_scope!("cull.shadow_views");    // scoped hierarchical timer, any thread
counter!(DRAW_CALLS, n);                // per-frame counter
gauge!(STREAMING_QUEUE_DEPTH, depth);   // sampled value
```

- Every execution context registers a **named thread lane**: Game, Render, game-pool workers, render-pool workers, Streaming Coordinator, Audio, IO pool.
- **Zero-cost rule.** All macros compile to no-ops in shipping builds; in dev builds, overhead is nanoseconds per scope with no client attached.
- Games use the identical macros — a game system's scopes appear in the same lanes and overlay as engine scopes.

### Sinks

| Sink                         | Role                                                                                   | When                      |
| ---------------------------- | -------------------------------------------------------------------------------------- | ------------------------- |
| **Tracy**                    | Deep dives: zones, lock contention, memory (alloc hook), GPU lanes, capture files      | Dev machine, on attach    |
| **egui overlay**             | Always available: frame graph, per-thread ms, top-N scopes, counters, **budget table** | Any dev build, toggle key |
| **chrome-trace JSON export** | CI captures, bug reports, offline diffing                                              | On demand                 |

- **GPU timeline unification.** `GpuTimingFeedback`'s per-pass timings — already frame-stamped by the readback ring — feed Tracy's GPU context, so CPU and GPU lanes sit on one timeline.
- Shipping telemetry (player-machine aggregation) is out of scope for v1; the trace export is the hook it would later build on.

### The BudgetRegistry — receipts for every budget

Every named budget in this document (GPU pools, staging pool, deform output, GI clipmap, froxels, streaming IO/upload/decode) **registers a gauge at creation**: budget, current usage, peak. The overlay's budget table renders whatever is registered — adding a budget without receipts is structurally impossible. Budget-consuming systems (the DRS controller, the streaming arbiter, deform LOD capping) read the same gauges they publish.

### The Standard Counter Set

Spec'd so tooling can rely on them: draw calls and instances per view; culled counts per view (mirroring the `OcclusionFeedback` advisory — that remains the game-facing data path); brick residency (resident / pinned / evicted this frame); streaming queue depth and in-flight bytes; staging in-flight regions; TLAS instance count; deformed instance count. Games add their own counters through the same macro.
