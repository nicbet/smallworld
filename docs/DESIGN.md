# smallworld — Technical Design

**Status:** Draft for review · **Issue:** sw-3a604e · **Date:** 2026-08-05

A micro-voxel engine adopting the load-bearing ideas from Nanite and Lumen — the error
metric, the watertight LOD hierarchy, GPU-driven culling, and hybrid screen/world-space
GI — re-expressed on voxel-native data structures, where several of those ideas become
simpler or free. Target: the technical foundation for a hi-definition-Minecraft-meets-WoW
game (destructible world, digging, mining, later NPCs/quests/inventory).

This document is the canonical definition of terms used across the xpo board (brick pool,
op log, TLAS, anchors, hot/cold). `notes.md` is background: measured analysis of a prior
Godot meshing experiment whose conclusions this design inherits.

---

## 1. Decisions

| # | Decision | Choice | Rationale | Revisit if |
|---|----------|--------|-----------|------------|
| D1 | Engine base | **Custom** (no Godot/Unity/Unreal) | The renderer is ~90% of the engine's value and precisely the part existing engines don't cede: the prior Godot experiment concluded the software-rasterizer/indirect-draw class of techniques "need the render loop we don't own." Nanite itself is a baked triangle pipeline — weakest exactly at our core requirement (real-time volumetric editing). | Goal shifts from building tech to shipping a game fast. |
| D2 | Language / graphics | **Rust + wgpu** (user decision) | One codebase → native Metal on macOS, Vulkan on Windows/Linux; no MoltenVK. Native extensions cover push constants and 64-bit atomic min/max. Memory safety in the multithreaded streaming core. rapier physics in-ecosystem. | wgpu blocks a required feature (see Risks). |
| D3 | Base voxel scale | **10 cm** (user decision) | Teardown-class, proven; cheapest memory/streaming; fastest path to playable. Per-instance voxel scale (D5) is the upgrade path to finer props without changing terrain cost. | World reads as too chunky after M2; then finer per-instance scale first, smaller base voxels last. |
| D4 | Storage | **Brick pool primary, SVDAG as background compression** | Mutable 16³ bricks are editable in O(edit volume); DAG compaction (70–90% reduction) applies only to cold regions and un-merges on edit. Avoids SVDAG's known edit problem (notes.md) without giving up its compression. | — |
| D5 | Scene model | **Terrain volume + voxel object instances (two-level TLAS/BLAS)** | Makes destruction per-object (bounded connectivity checks), lets debris render through the same raymarcher under a transform, and gives per-instance voxel scale. | — |
| D6 | Rendering | **GPU raymarching, no meshes** | Crack-free LOD by construction; "never do sub-pixel work" becomes a traversal-termination rule instead of a mesh-swap policy. Eliminates the two measured failure modes of the meshing experiment (LOD cracks, sub-pixel quads). | — |
| D7 | Persistence | **Procedural base + compressed DAG + per-region op log** | Player edits stored as operations; saves proportional to what changed; lazy full-res replay enables cheap distant edits. | — |
| D8 | Terrain collapse | **Terrain exempt from structural integrity** (Minecraft rule) | Keeps connectivity per-object and bounded. Cave-ins are a deferred game-design knob, not an engine gap. | Game design wants mining hazards. |
| D9 | Water | **Fluid-as-material now, sim state later** (review round 1) | Still water is an ordinary voxel material — generated, DAG-compressed, persisted through existing paths, reserved in the M1 data model at near-zero cost. Flowing water promotes bricks into an active-sim set with an auxiliary level/flow channel; simulation is a dedicated late epic. | — |
| D10 | Atmosphere | **Participating media on the primary march** (review round 1) | Fog and god rays are accumulation along the ray march we already perform — no froxel volumes, no separate system. Global weather state (wind, fog, precipitation, time-of-day) plumbed as uniforms from M0. | — |

Commodity dependencies: `winit` (window/input), `egui` (debug UI), `rapier3d` (dynamic
bodies), WGSL shaders. Audio (`kira`) and ECS (possibly `bevy_ecs`) deferred to the game
layer. Non-goals for v1: networking, any mesh pipeline, mm-scale voxels.

---

## 2. World model

Two kinds of things exist:

- **Terrain** — one large, identity-transform volume. Editable, exempt from collapse.
- **Voxel object instances** — tree, tower, boulder, debris chunk, ore drop. Each owns a
  local brick tree, a rigid transform, and a **per-instance voxel scale** (a 10 cm world
  can hold a 2.5 cm-voxel chair). Objects have **anchors** (§7) and participate in
  structural integrity.

A small BVH (**TLAS**) over instance AABBs is refit per frame. Rays traverse: TLAS →
transform into object space → the same brick DDA used for terrain. One traversal code
path renders everything, including falling debris.

## 3. Data structures

**Brick** = 16³ voxels = 1.6 m cube at base scale. Voxel = 8-bit index into a per-brick
material palette (bricks rarely exceed a handful of materials); 4 KB payload + palette.
Bricks live in a pooled GPU allocator with stable handles and free-list recycling.

**Top-level index** — maps space → brick handles. At 10 cm, a 4 km world is only
~2.5k bricks per axis, so a shallow structure (paged grid or 2–3-level tree) suffices;
no deep octree at the top level.

**Mip chain** — per-region downsampled occupancy + averaged material/color. Serves three
masters: SSE-terminated rendering (§4), cone-traced GI (§8), and coarse collision. Built at
generation, re-filtered upward on edit.

**Hot/cold:** the SVDAG is a *compression cache for immutable data*, not a separate world
format. Background compaction deduplicates quiet regions; any edit un-merges just the
affected root-to-leaf paths (O(depth) new nodes — Careil & Billeter 2020) back into pool
bricks, which stay resident until quiet again. Editability is governed by *heat*, never
by distance.

Bricks may carry optional **auxiliary channels**, allocated only where needed — the first
user is the fluid level/flow channel of the water active-sim set (§10).

## 4. Rendering

Fullscreen compute pass. Hierarchical DDA: coarse steps through empty space via the
index/mips, fine DDA inside bricks. Raymarching is inherently occlusion-culled — rays
stop at first hit — so Nanite's two-pass occlusion pipeline has no analog to build;
empty-space skipping is the perf primitive instead. The traversal loop is shaped so a
hit can continue as a transmission segment (water, §10) and so per-step accumulation
(fog, god rays) hangs off the same march — implemented later, structured from the start.

**LOD = traversal termination.** Stop descending when a node's projected footprint is
below ~1–2 px and shade from the mip. At 1080p (~935 px focal), a 10 cm voxel subtends
1 px at ~90 m: full resolution is only ever traversed in a ~100 m bubble, mip *n* covering
roughly twice the distance band of mip *n−1*, horizon at mip 5–6. No seams exist because
no meshes exist.

Shading v1: face normals from DDA hit axis (crisp voxel look), one sun shadow ray through
the same traversal. GPU timestamp queries around every pass from day one — every lesson
in notes.md came from a measurement.

## 5. Editing

CSG brushes (sphere/box subtract & place, material-aware) through copy-on-write bricks.
Cost bounded by edit volume: a 5 m blast touches a few hundred bricks (~1–2 MB), whether
it lands next to the player or 400 m away. Removed-voxel data is capturable by the caller —
this single hook feeds debris, ore drops, and island extraction.

After leaf edits, the parent mip chain re-filters upward (background thread) so coarse
LOD views show the hole — without this, distant edits exist in data but not on screen.
Collision proxies for dirty bricks rebuild on the same thread.

**Distant edits are nearly free:** apply the op to resident coarse mips immediately
(instant visual feedback at the rendered LOD), append it to the region's op log, and
replay at full resolution only when the player approaches.

## 6. Streaming & memory

Demand is driven by camera position + SSE: the streamer pages bricks (generate or load)
so that every visible region is resident *at the mip the SSE requires* — coarse mips for
the whole world stay resident (small); full-res bricks only near the player. Hard VRAM
budget (initial target ≤ 2 GB for world data ≈ 500 k bricks) with LRU eviction. Async,
multithreaded, hitch-free flight is the acceptance bar. This is the highest-risk
subsystem (sw-943a75).

## 7. Destruction & physics

Pipeline, all edit-driven, no per-case scripting:

1. **Connectivity summaries** — per brick: which faces connect through its interior.
   Structural questions run on the brick adjacency graph (thousands of nodes), descending
   to voxel level only inside bricks the cut touched. Recomputed only for dirty bricks.
2. **Island detection** — async (1–3 frames; 30–50 ms is imperceptible): flood from both
   sides of the cut; the side that cannot reach the object's anchor is detached.
3. **Extraction** — detached voxels become a new object instance (own grid + origin),
   subtracted from the source via the standard edit op. Mass, COM, inertia are exact sums
   over occupancy × material density.
4. **Simulation** — rapier rigid bodies. Colliders: rapier voxel colliders if current,
   else greedy-merged box compounds from coarse mips. No scripted topple: contact between
   the falling tree top and the stump produces the tip-and-slide.
5. **Settling** — sleeping instances, never re-voxelized (rotated grids resample lossily);
   small debris coarsens/despawns.

**Anchors** — object base bricks contacting solid terrain. Terrain edits that dirty bricks
intersecting an anchor region queue a re-anchor check: tunneling under the castle wall
collapses it through this same pipeline. Undermining is emergent, not scripted.

## 8. Lighting

Lumen's architecture, cheaper on voxels — its SDF volumes and surface cache exist to
*approximate a triangle scene as a volume*; our scene already is one, with material/color
in the mips:

1. Screen-space ray trace first (detail, visible geometry).
2. World-space fallback: cone tracing through the brick mip chain (AO + sky occlusion
   first; then one-bounce diffuse GI with temporal accumulation).
3. Emissive as a material property (ore glow, lava, torches), HDR + filmic tonemap +
   auto-exposure for cave↔surface.

Because GI traces live data, dug tunnels go dark and opened walls let light in with zero
light-propagation code — Minecraft's flood-fill lighting subsystem simply doesn't exist here.

## 9. World generation

A deterministic 3D density + material function (not a heightmap): surface heightfield
blended with stone strata, cave carving, and vein-shaped ore noise with depth-dependent
distribution. A water table places oceans, ponds, and slow rivers as still-water material
(§10) so water exists from the first generated world. Generates bricks on demand from
seed; underground must contain things worth digging for. Ore mining = material-filtered subtract; the removed voxels *become* the
collectible chunk body (the item you pick up is literally what you mined; its voxel data
doubles as the item model/icon).

## 10. Water & atmosphere

**Water — representation early, simulation late (D9).** Still water (oceans, ponds,
lakes, slow rivers) is an ordinary voxel material: the generator's water table places it,
the DAG compresses it, the op log persists edits to it. Rendering adds one **transmission
segment** to the raymarcher: refract at the surface, continue the march, tint by traversed
depth (Beer–Lambert), Fresnel-blend reflections from the lighting stack (screen-space
first, cone-traced fallback). Flowing water (waterfalls, currents, basin filling) promotes
affected bricks into an **active-sim set** carrying an auxiliary level/flow channel — the
hot/cold pattern again: extra state only where water moves, GPU cellular update, demotion
back to plain material on settling. Digging a channel to a pond promotes the boundary
bricks and the channel fills. This is notes.md's "GPU fluid dynamics via cellular
occupancy".

**Atmosphere — mostly free by construction (D10).** Volumetric fog and god rays are
accumulation along the primary march we already perform: sample fog density and sun
visibility (through coarse mips) while walking the ray. Heterogeneous, voxel-shadowed
light shafts fall out of the renderer's shape — no froxel volumes, no post-process shaft
hack. Clouds are a raymarched layer whose coverage feeds the GI sky term; rain is
particles + material wetness + puddles (shallow still water in depressions); wind is a
global field consumed by particles, wave normals, and per-instance vegetation sway (the
TLAS already refits per frame). Global weather state (wind, fog, precipitation,
time-of-day) is plumbed as engine uniforms from M0.

## 11. What we take from Nanite / Lumen

| Their idea | Their mechanism | Ours |
|---|---|---|
| Constant cost vs density | DAG of 128-tri clusters + SSE selection | SSE-terminated tree traversal; parents are filtered children |
| Watertight LOD seams | Locked cluster boundaries | Free — no meshes, no seams |
| Never shade sub-pixel | Software rasterizer fallback | Never *traverse* sub-pixel; shade from mips |
| Occlusion culling | Two-pass HZB reprojection | Inherent — rays stop at first hit |
| GI acceleration | SDF proxy + surface cache | The voxel mip chain *is* both |
| Screen-space first | SSRT → SDF fallback | Same, cone-traced mips as fallback |

## 12. Milestones (= epics on the board)

| M | Epic | Exit criterion |
|---|------|----------------|
| M0 | sw-4df655 Foundation & Toolchain | Dense 256³ brick raymarched on Metal + Vulkan, GPU timers on screen |
| M1 | sw-aa609f Sparse World & Core Raymarcher | Free-fly a generated sparse world with sun shadows, zero meshes |
| M2 | sw-b254c8 Scene Structure (objects + TLAS) | Hundreds of instances via the same traversal, movable at runtime |
| M3 | sw-d6d2c2 Streaming & LOD | km-scale world, hitch-free, fixed budget, no sub-pixel traversal |
| M4 | sw-d9e5c0 Editing & Persistence | Dig a tunnel at 60 fps; reload; it persists |
| M5 | sw-b815a0 Destruction & Physics | Tree top falls when chopped; wall debris; undermining collapses |
| M6 | sw-c13774 Lighting & GI | Tunnels go dark naturally; GI responds to edits |
| M7 | sw-b146f7 Materials, Digging & Ore | Mine a vein, chunk drops, pick it up, survives reload |
| M8 | sw-0656de Water | Swim in a pond with refraction/reflections; waterfall flows; a dug channel fills |
| M9 | sw-7dd08f Atmosphere & Weather | Foggy dawn with god rays; rain wets surfaces and puddles; clouds darken GI |

M3↔M4 order is soft: editing (M4) can begin against the un-compacted brick world once
M3's mip chain exists; only DAG un-merge depends on compaction. M8/M9 are parallel tracks
once M6's lighting stack exists, and the fog/god-ray stories share M6's machinery and can
land alongside it. Water's *representation* is deliberately not deferred: it is reserved
in M1 (bricks sw-3afd48, traversal sw-30aa3c, generator sw-00ae86) per D9.

## 13. Risks & deferred knobs

**Risks**
- **Streaming (sw-943a75)** — the hardest single story; everything at km scale depends on it.
- **wgpu portability** — timestamp queries, subgroup ops, and 64-bit atomics differ per
  backend; validate on Metal early (M0 exists partly for this).
- **rapier voxel colliders** — verify current status at implementation (sw-d6b1da); box
  compounds are the fallback.
- **GI temporal stability** — accumulation vs. edit responsiveness is a known tension (sw-d3ae86).
- **Transmission cost** — the underwater continuation lengthens the march in water-heavy
  views (sw-cf89be); needs a step budget and cap.
- **TLAS scale** — hundreds of instances is designed-for; tens of thousands (forests) may
  need instance LOD/impostors later.

**Deferred, deliberately**
- Terrain cave-ins (D8) — coarse brick-level support rule when game design wants it.
- Sub-10 cm props via per-instance scale — supported by D5, exercised when art needs it.
- Game layer (ECS, inventory UI, NPCs, quests) — separate design doc when M7 nears.
- Heat/temperature simulation (indoor warmth retention — notes.md pointer) — post-M9.

## 14. References

- `notes.md` — measured conclusions from the prior Godot meshing experiment.
- Karis et al., *Nanite — A Deep Dive*, SIGGRAPH 2021.
- Wright et al., *Lumen: Real-time Global Illumination*, SIGGRAPH 2022.
- Careil, Billeter, Eisemann, *Interactively Modifying Compressed Sparse Voxel
  Representations*, Eurographics 2020 — real-time SVDAG editing.
- Kämpe, Sintorn, Assarsson, *High Resolution Sparse Voxel DAGs*, SIGGRAPH 2013.
- Laine, Karras, *Efficient Sparse Voxel Octrees*, I3D 2010.
- Crassin et al., *GigaVoxels*, I3D 2009 — hierarchical GPU voxel raymarching.
- Gustafsson, Teardown engine talks/blog — object-decomposed voxel destruction.
