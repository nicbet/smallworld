## Vegetation & PCG

_(OQ 29 resolution, 2026-08-11.)_ Engine/game split: the engine owns the PCG framework, the `ScatterSurface` sampler trait, and the foliage renderer; games author scatter graphs and rules; surface providers plug in (heightfields, meshes, the Voxel Plugin via its V6 sampler).

### The PCG Framework

Graph-based scatter — sample → filter → spawn — authored as serde data (the data-graph pattern's fifth appearance: `AnimGraph`, `MixerLayout`, density graphs, now scatter graphs):

- **Runs at cell-generation time in the streaming pipeline** — coordinator-dispatched, worker-executed, cached as cell content per OQ 17, invalidated per the sampler provider's rules (the Voxel Plugin's pristine/edited split, for instance). Deterministic by construction: seeded by `(cell coord, graph hash)`.
- **Node classes:** point generators (Poisson-disk, jittered grid, cluster), surface sampling (project + attributes via `ScatterSurface`), filters (slope/height/material/channel predicates, noise masks), transforms (gravity/normal alignment blend, scale/rotation ranges), spawners (weighted prototype sets → an output tier).
- **Deterministic thinning.** Every instance carries a stable random key from generation; density settings and distance falloff show instances with `key < threshold` — quality sliders and falloff bands never re-scatter, they move a threshold.

```rust
trait ScatterSurface {
    // Project a point along a direction (usually local gravity) onto the surface.
    fn project(&self, origin: Vec3, dir: Vec3) -> Option<SurfaceSample>;
}

struct SurfaceSample {
    position: Vec3,
    normal:   Vec3,
    material: u16,
    channels: ChannelValues,   // named scalars: vegetation_density, moisture, …
}
```

### Two Output Tiers

Chosen per prototype in the graph — the UE actors-vs-HISM / Unity trees-vs-details split; every engine lands here because the two cost models are irreconcilable:

- **Entity tier** (trees, rocks, interactables): spawned through cell documents — identity, colliders, behaviors, overlay persistence. Bounded counts.
- **Instance tier** (grass, pebbles, clutter): **no entities at all** — per-cell instance sets feeding the renderer directly. Interaction happens via overlay removal-marks keyed by the stable instance key, never via entity identity.

### Foliage Rendering

- **Per-cell, per-prototype instance batches on the shared mesh stream.** One `MeshDrawCommand` per (cell × prototype); instances pre-baked into the shared `InstanceData` buffer at cell load; culling is **cell-granular** — one AABB per batch — which keeps v1's CPU culling viable because instance-tier content exists only in the near ring (thinning-key falloff). Shadows per prototype (`CAST_SHADOW`: grass off, trees on); LOD fades via the OQ 10 dither; HZB occlusion of whole cells. **Foliage is the named first customer of OQ 7's GPU-driven phase 2** — per-instance GPU culling arrives there, additively.
- **Far tier: cook-time imposters** — octahedral or cross-billboard, generated per prototype by the OQ 27 pipeline as derived data, swapped via dithered fade. Per-cell merged HLOD imposters slot into OQ 17's reserved HLOD contract later.
- **Wind: material-level vertex animation.** Global `WindParams` (direction, strength, gustiness) in `EnvironmentParams`, read by a vertex-stage `ShaderFragment` through the custom-material system; per-instance phase from the stable key; branch weights painted in vertex colors for trees. Deliberately _not_ the DeformPass — per-instance skinned buffers are the wrong cost model for a million grass blades.

### Persistence

Dictated by OQ 17/20: generated scatter is cell cache (regenerable from the density-graph × scatter-graph hashes); player modifications are overlay — removal-marks for the instance tier, entity overlay for the heavy tier.
