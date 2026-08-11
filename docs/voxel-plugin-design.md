# Smallworld Voxel Plugin — Architecture Design

Companion to `architecture-design.md`. The engine treats voxel support as the **Voxel Plugin** —
built entirely on public contracts ("if the API isn't powerful enough for voxels, it isn't
powerful enough for games"). This document designs the plugin itself. Working method: the
V-series open questions below are resolved one at a time in discussion, and each resolution is
written back into this document as a decision record — the same process that produced the engine
doc's OQ rounds.

---

## Inherited Contracts (decided engine-side; the plugin builds on these)

| Contract | Engine spec | Plugin obligation |
|----------|-------------|-------------------|
| Backend split | `GeometryExtractor` + `GeometryRenderer`, custom lane + shared mesh stream | Extracted-mesh tiers ride the shared stream; raymarch data rides the custom lane |
| Rendering mechanism | Volume Rendering Mechanism (OQ 1): proxy-raster fragment raymarch, `frag_depth`, velocity, early-out vs `depth_mesh_copy` | Implement brick/SVO DDA in the raymarch shader |
| Shadows | `ShadowCaster` participation trait (OQ 1) | Depth-only raymarch variant per shadow view |
| LOD transitions | Fade/dither public convention + residency gating (OQ 10) | Distance-banded dual-LOD blending (private); extract-from-same-LOD convergence at the handoff |
| Streaming | Demand/fulfill, residency truth with the brick pool, pinned-coarse invariant, region files (OQ 17/21) | Demand hints only; coarser-parent fallback in the shader |
| Content sources | `VolumeSource` registry, `GenerationPolicy`, purity rule (OQ 17/21) | Sources are pure over `(params, coord)`; params are plain data |
| Lighting integration | Public GI / shadow-mask / sky-visibility slots; GI injection point; froxel injection (OQ 2/4/9/11) | SVO compute tracing feeds the slots; SVO data injects into the GI clipmap; media injects into froxels |
| Media split | Solid volumes ≠ participating media (OQ 9) | `VolumePass` never renders media; smoke/fire = froxel far tier + raymarched media pass near tier |
| Future upgrade | Compute visibility-buffer tier, capability-gated (OQ 1/7) | Deferred until GPU-driven phase 2; triggers recorded engine-side |
| Serialization | Sources referenced by stable name; edits in cell overlays, never cache (OQ 17/20/21) | Region/overlay formats are plugin-defined within engine file conventions (`user://`, cell naming) |

---

## Open Questions (V-Series)

- **V1. [RESOLVED 2026-08-11] Voxel topology** — this plugin is cubic; hex/Goldberg-sphere
  planets are a committed sibling plugin on the same engine contracts. See Resolutions.
- **V2. [RESOLVED 2026-08-11] World topology** — flat and spherical both, natively: topology
  is content, not lattice. See Resolutions.
- **V3. [RESOLVED 2026-08-11] Surface representation** — `SurfaceMode::Blocky | Smooth` as a
  per-volume checkbox; smooth extraction = MC (MC33) + Transvoxel. See Resolutions.
- **V4. [RESOLVED 2026-08-11] Shape composition** — two levels: the `VolumeSource` trait as
  the primitive level, **density graph assets** (`GraphSource`) as the data-driven authoring
  level, batch-evaluated per brick. See Resolutions.
- **V5. [RESOLVED 2026-08-11] Material model** — layered assignment (`AUTO` sentinel + rules,
  stored overrides), stochastic dithered transitions under the OQ 10 convention, per-volume
  palette with u16 namespace + per-brick palette compression. See Resolutions.
- **V6. [RESOLVED 2026-08-11] Vegetation & detail placement** — the plugin implements the
  engine's `ScatterSurface` sampler: analytic graph sampling for pristine cells, realized-voxel
  fallback where edited; gravity-aligned frames. See Resolutions.
- **V7. [RESOLVED 2026-08-11] Voxel data layout** — 16³ bricks at every level;
  **demand-driven adaptive depth** (no fixed leaf level); per-brick variable-width palettes;
  pristine-generates / edited-filters. See Resolutions.
- **V8. [RESOLVED 2026-08-11] Editing & destruction** — brush/op API with an edit budget and
  an explicit six-way propagation fan-out with latency classes. See Resolutions.
- **V9. [RESOLVED 2026-08-11] Physics representation** — collision-only extraction ring,
  the under-your-feet priority rule, analytic far-field raycasts. See Resolutions.

---

## Resolutions

### V1 — Cubic Lattice Here; Hexasphere as a Sibling Plugin *(resolved 2026-08-11)*

**This plugin is cubic.** Everything in it is cubic-lattice machinery: octree/SVO subdivision,
closed-form (x, y, z) brick addressing, hierarchical DDA raymarching, MC/DC-family extraction
tables, clipmap cascades.

**Hex/Goldberg-sphere planets are a committed sibling plugin** — scoped, not scheduled — and
not a lattice parameter of this one. The Goldberg approach (hexagons + the twelve mandatory
pentagons — Euler's formula makes them unavoidable — extruded radially into shell-stacked prism
columns) legitimately solves uniform sphere tiling and tile-adjacency gameplay. But it shares
almost no algorithms with the cubic plugin: a Goldberg lattice is a *graph* (precomputed
adjacency tables; no uniform indexing — the twelve pentagons guarantee that), not a grid.
Generalizing one plugin over both lattices was rejected as the union-interface anti-pattern
(the engine's physics-provider rule, applied to ourselves).

Why the sibling is cheap — the plugin thesis paying out again:

- **Gameplay-scale tiles, not microvoxels.** Hex-planet cells are legible gameplay tiles;
  rendering is greedy-meshed prism geometry on the **shared mesh stream** — shadows, HZB,
  TLAS, and LOD fades arrive free through existing engine contracts; likely no raymarching at
  all. Column-shell storage plus static adjacency tables, and it's done. A smooth variant
  stays possible via tetrahedral decomposition (marching tets) if ever wanted.
- **Together, the two plugins span the round-planet space on the aesthetic axis:** smooth
  planets = this plugin with radial density (V2); tiled planets = the hexasphere sibling. The
  choice is an art/gameplay decision per title, never an engine mode.
- **Shared where genuinely coincident:** the source-registry pattern, the material-model
  philosophy, and the PCG sampler hooks (V6 / engine OQ 29).

### V2 — World Topology Is Content, Not Lattice *(resolved 2026-08-11)*

Flat and spherical worlds are **both supported natively, by the same plugin**, because topology
lives in the density source, not the lattice — Astroneer's shipped proof. A planet is a *radial*
density source (sphere-distance minus surface noise) evaluated in an ordinary cubic lattice
large enough to contain it; a flat world is a *planar* source in the same lattice. The plugin
has no "world type"; its `VolumeSource` stack (V4) does. Consequences:

- **Gravity is a field query on the source** (radial vs. constant) — consumed game-side, like
  root motion: delivered, never imposed.
- **Streaming** uses the engine's 3D cell-grid option for planets (2D default for flat worlds) —
  World Streaming already supports both by config.
- **LOD and demand metrics are Euclidean either way** — unchanged.
- **Sphere-tiling lattices (cube-sphere quadtrees, radial prism shells) are rejected for this
  plugin** — every algorithm would inherit their distortion forever. Tiled-planet *aesthetics*
  are a different question: see V1's revisit.

**Rider (engine-sized):** planet-scale worlds break `f32` — visible transform jitter beyond
~10 km from origin. Escalated to **engine OQ 30 (large-world coordinates)**: f64 world space +
camera-relative rendering touches `WorldTransform`, extract, physics, and streaming — engine
contracts, not plugin ones.

### V3 — Surface Mode Is a Checkbox; Smooth = MC + Transvoxel *(resolved 2026-08-11)*

**`SurfaceMode::Blocky | Smooth`, per volume** — the UE Voxel Plugin precedent, where
blocky/smooth is literally a checkbox. The modes share everything except surface evaluation:
bricks, SVO, streaming, residency, edits, materials, and region files are identical; only the
raymarch inner loop differs (integer DDA face-hit vs. trilinear isosurface march) and the
extractor (greedy meshing vs. isosurface extraction). Two shader variants and two extractors
over one data pipeline — genuinely shared substrate, not the union-interface trap V1 rejected.
Games pick per volume and may mix (blocky build-zones in a smooth world). Blocky's raymarch
loop is the cheaper of the two.

**Smooth extraction: Marching Cubes (with MC33 ambiguity resolution) + Transvoxel LOD
stitching.** Two decisive grounds:

1. **LOD seams are the load-bearing case.** The far tier is concentric extraction rings by
   construction — V2 planets make ring boundaries ubiquitous — and Transvoxel's
   transition-cell tables are the only industrial-strength crack-free answer; both dual
   methods improvise here.
2. **Convergence with the raymarcher.** The raymarched surface *is* the trilinear isosurface,
   and MC's vertices lie exactly on it (edge crossings of the same interpolant) — the
   extracted mesh chords the very surface the near tier renders, which is precisely what the
   OQ 10 handoff convergence rule requires.

**Rejected: Dual Contouring** — its sharp-feature payoff needs analytic Hermite data, but the
convergence rule feeds extraction from *sampled brick LODs* (finite-difference gradients),
washing features out at far-tier resolution while DC's costs remain (QEF solves, manifold
robustness, no Transvoxel equivalent for LOD stitching). **Fallback noted: Surface Nets**, if
Transvoxel's transition cells ever prove more trouble than their worth.

**Forward note (V5):** MC edge-vertices interpolate between two voxel corners — exactly where
per-corner material IDs blend; this choice feeds the dithered-material design directly.

### V4 — Density Graphs Over Source Primitives *(resolved 2026-08-11)*

The fourth appearance of the engine's two-level pattern (pose primitives → `AnimGraph`; mixer →
`MixerLayout`; scatter → PCG graphs): **the `VolumeSource` trait is the primitive level; a
`GraphSource` implementing that trait is the data level.** Code sources stay first-class for
the exotic; **density graph assets are the standard authoring path** — a DAG of primitive
nodes (plane, sphere, box), noise nodes (FBM simplex, ridged, billow, Worley, domain warp),
operators (smooth-min CSG union/subtract/intersect, remap curves, clamps, masks/selectors),
and domain transforms, serialized like every other graph asset. The UE Voxel Plugin's Voxel
Graph model, on our contracts.

- **Batch-per-brick evaluation, not per-voxel.** Each node processes the entire brick's sample
  buffer in one call (array-programming style, 512+ voxels per dispatch) — interpreter
  overhead amortizes to nothing; loops are SIMD-friendly and cache-coherent (the FastNoise2
  model). Compilation/JIT stays a someday-optimization, not a prerequisite.
- **Cache integration is automatic.** Graphs are pure over `(params, coord)` per the existing
  contract, and the **graph content hash participates in the source-version cache key** — edit
  a node and OQ 17's `CacheToDisk` invalidation is precise and automatic. Graph assets
  hot-reload in dev; visible bricks regenerate.
- **Graphs output more than density**: output nodes for **density**, **material ID** (feeding
  V5 — slope-masked material selection is authorable in-graph), and **named scalar channels**
  (`vegetation_density`, `moisture`, …) — exactly what the PCG sampler (V6 / engine OQ 29)
  queries. One graph authors shape, surface, and ecology inputs.
- **LOD consistency is inherited, not engineered:** a pure function sampled at coarser spacing
  *is* the coarser LOD — the V3 convergence rule and the pinned-coarse fallback lean on this
  with zero further machinery.
- **V2 compliance by construction:** a planet is a radial primitive + noise layers; a flat
  world is a plane primitive + the same layers. Topology-as-content, now authorable.

### V5 — Layered Materials, Dithered Transitions *(resolved 2026-08-11)*

**Assignment is layered: procedural default, stored override.** The per-voxel material field
has an **`AUTO` sentinel**: `AUTO` voxels get their material from a **`MaterialRules` asset**
evaluated at surface time — normal/slope, world height/depth, curvature, noise break-up — while
explicit IDs (painted, built, or written by V4 graph material-output nodes) override locally.
The rule function is one shared implementation evaluated identically in the raymarch shader and
the MC extractor, so both tiers agree by construction. Memory-light (most of a world is
`AUTO`), live-tunable (rules re-tune without regeneration), and paintable where it matters.
Stored-only was rejected (memory for derivable data, frozen rules); procedural-only was
rejected (player edits inexpressible). This is roughly where the UE Voxel Plugin converged.

**Transitions are stochastic dithered selection — the OQ 10 dither convention applied to
materials.** Per pixel, hash a threshold against the blend weight and sample **exactly one**
material; TAA resolves the stipple into a perceptually smooth transition. One material sample
per pixel regardless of how many materials meet, applied uniformly to MC corner-blend weights
(V3's forward note) and to rule-boundary bands (soft thresholds dither instead of hard-edging).
N-way splat blending rejected (N× texture cost, smeared contacts). Weight-dependent effects
(depth-blended snow) remain authorable as rules, which run pre-selection.

**Appearance: a per-volume material palette asset** — an array of entries (albedo / normal /
roughness sets + params) bound as texture arrays. Smooth mode shades with **triplanar
projection** (voxel surfaces have no UVs; stochastic tiling later to break repetition); blocky
mode uses per-face atlas UVs. Both paths ride existing engine contracts: `VolumePass` writes
the GBuffer directly; extracted tiers use a custom material (triplanar fragment, `Standard`
shading model) via the shader-composition system.

**Namespace: u16 volume-level material IDs** (65 k per volume — biome-rich planets never
sweat it; u8 was judged too close to the ceiling), with **per-brick palettes** doing the
compression: a brick stores a small palette of the materials actually present, voxel indices
are 2–6 bits into it, palette entries are u16 volume IDs. Namespace generosity and storage
compactness never compete. Storage details: V7.

### V6 — Scatter Sampling: Analytic for Pristine, Voxels Where Edited *(resolved 2026-08-11)*

The engine's PCG framework (OQ 29) owns point-pattern generation, rule filtering, and instance
spawning into cells; the plugin implements the **`ScatterSurface` sampler** — "where is the
surface, and what is it like there." The core decision:

- **Pristine cells sample the pure density graph analytically.** V4's purity pays out: surface
  position along a gravity ray is a **root-find on a pure function** — no geometry, no bricks,
  no residency, runnable at cell-generation time on any thread. Normal = gradient; material =
  the same shared rule function (V5); ecology inputs = the graph's named channels
  (`vegetation_density`, `moisture`) sampled directly. Scatter results are **cell-document
  cache content**, keyed by (density-graph hash × scatter-graph hash) — regenerable, and
  computed entirely off the hot path.
- **Edited cells fall back to realized voxel data.** Overlays diverge from the graph (trees
  must not float over dug pits): cells with edit overlays re-scatter *locally* against actual
  voxel data in the edited footprint, and terrain edits **invalidate scattered instances in
  their footprint**. Player-removed vegetation (the chopped tree) is itself overlay truth (OQ
  17: edits are never cache) and survives re-scatter.
- Voxel-data-only sampling was rejected (it would make scattering depend on brick residency —
  backwards: a cell's vegetation should be ready when the *cell* loads); graph-only was
  rejected (floats over player edits).

**The scatter frame is gravity-aligned, not world-Y.** "Up" is a gravity-field query (V2): on a
planet the frame is radial — vegetation grows away from the core — and **slope is measured
against local gravity**, so a cliff near the pole is still a cliff. One field query, and the
ecology bends around the planet correctly.

Spawned instances are cell content on the shared mesh stream; foliage rendering at scale
(imposters, wind, density scaling) is the engine's side (OQ 29). Forward note: the sampler
never says "plant" — the same hooks serve rocks, debris, and ruin placement for free.

### V7 — 16³ Bricks, Demand-Driven Adaptive Depth *(resolved 2026-08-11)*

- **16³ bricks at every level** — a benchmark-validated compile-time constant (8³ drowns in
  per-brick metadata and fine-grained streaming traffic; 32³ makes streaming and edit
  granularity lumpy; 4,096 voxels is the sweet spot both axes tolerate).
- **No fixed leaf depth — tree depth is demand-driven** *(revised in discussion: the original
  draft assumed a fixed finest level; the structure doesn't need one)*. Any node may be
  realized as a brick at its own scale; subdivision deepens where LOD demand asks — the
  OQ 10/17 demand rings translate directly into tree depth — bounded only by a per-volume
  `max_depth` (finest voxel size, from the source's `lod_hint`). The near field reaches
  microvoxel fineness; a planet's far side never realizes deep levels. **Resolution is bounded
  by demand and configuration, never by structure.**
- **Pristine coarse bricks are generated, not averaged.** V4 purity means the source evaluates
  at *any* sample spacing, so a coarse brick is generated directly at its level's spacing —
  higher quality than filtering, and correct without children existing, which is what makes
  the pinned tier cheap and self-sufficient. **Edited regions filter upward instead**: edit
  truth lives at the finest edited level and propagates up by filtering — the pristine/edited
  split, a third time (V6, V9).
- **Payload:** smooth = quantized density (u8, narrow band around the isosurface) + palette
  index; blocky = block ID (occupancy ≡ ID ≠ air). **Per-brick palettes with variable
  bit-width** (1–8 bits by distinct-material count; entries are u16 volume IDs per V5) — the
  Minecraft paletted-container scheme, massively proven.
- **Homogeneous collapse:** all-air / single-material nodes store as constants in the tree, no
  brick allocation — the "sparse" in SVO.
- **Residency:** realized bricks at any level stream through demand/fulfill; **the pinned tier
  is all levels above a fixed shallow depth** — small, always resident, self-sufficient via
  direct generation. **CPU residency ⊆ GPU residency:** near-gameplay bricks keep CPU copies
  (edits, physics, queries); far pristine bricks are GPU-only and regenerable — the
  GPU-is-a-cache invariant applied inside the plugin. Overlay files store edited bricks in the
  same encoding, zstd-compressed, in OQ 17's region files.

### V8 — Editing: Brush Ops + the Propagation Fan-Out *(resolved 2026-08-11)*

**API: batched brush ops** — `edit(volume, Brush::Sphere{..} | Box{..} | Capsule{..},
Op::Dig | Place(material) | Paint(material))` — CSG-shaped brushes applied to CPU-resident
bricks at a defined phase (end of Update), under an **edit budget**: large events (explosions)
queue and amortize across frames instead of spiking.

**The spec's real content is the fan-out.** One edit touches six systems, each with a stated
latency class — written down so destruction is predictable instead of a bug farm:

| Propagation | Latency class |
|-------------|---------------|
| GPU brick re-upload (staging path) | Same frame |
| Local re-mesh of extracted tiers (worker pool, dither-faded per OQ 10) | Async, 1–3 frames |
| GI clipmap re-voxelization of the touched region (injection point) | Same frame |
| Collider rebuild (V9) | Priority-tiered — see the under-your-feet rule |
| Overlay persistence write (via the Streaming Coordinator) | Lazy |
| Scatter invalidation in the footprint (V6) | Async |

**Determinism note:** edits land in Update and propagate async; under future netcode, edit
determinism becomes its own question — parked with the OQ 19 era, not solved now.

### V9 — Physics: the Collision Ring *(resolved 2026-08-11)*

- **Collision-only extraction ring.** Terrain colliders exist only where physics activity is: a
  small radius around characters and awake dynamic bodies gets per-brick-region colliders —
  trimesh from the *same MC implementation* (smooth) or greedy box-compounds (blocky) — as
  static bodies through the standard provider API. No provider-specific voxel shapes: that
  would break OQ 16's portability rule. The shipped-voxel-game standard.
- **The under-your-feet rule.** Collider rebuilds for brick regions overlapping character or
  dynamic-body AABBs are **top priority and complete before the next fixed tick** — dig under
  yourself and you fall; you never phase through.
- **Far-field queries go analytic.** Beyond the collider ring, gameplay raycasts against
  terrain resolve by **root-finding the pure density graph** (V6's mechanism, reused) —
  pristine cells are shootable with no geometry at all; edited regions query voxel data. The
  pristine/edited split closes its loop.
