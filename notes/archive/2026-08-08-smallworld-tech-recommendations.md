# smallworld — technique selection

A pass over the full candidate menu (104 techniques, 12 domains), judged against
`DESIGN.md` rather than against what UE5/Unity/Godot happen to ship.

**Verdicts**

- **Take** — adopt substantially as the reference engines do it
- **Adapt** — same algorithm, materially changed by the SVO, the hybrid GBuffer, or wgpu
- **Skip** — doesn't apply here, with the reason it doesn't
- **Open** — a genuine fork; needs a prototype or benchmark, not an argument

## Platform baseline — wgpu 30

Pinned: `wgpu = "30"`, `winit = "0.30"`, `egui = "0.36"`, `glam = "0.33"`.

What wgpu 30 does and doesn't give you, since it decides more entries than any
engine comparison does:

| Capability | Status at wgpu 30 | Consequence |
|---|---|---|
| Mesh shaders | Available since v28. Full on Vulkan; Metal and DX12 via passthrough shaders | The meshlet path is no longer confined to compute + indirect draw — but passthrough means backend-specific shader code on two of three backends |
| Hardware ray tracing | `Features::EXPERIMENTAL_RAY_QUERY`. Inline ray query only — no RT pipelines, no shader binding table. Documented as subject to breaking changes | Usable, but not as a foundation you can't fall back from |
| AABB BLAS (procedural geometry) | Added in v30 | Brick bounds can go into a hardware acceleration structure; RT cores traverse to brick granularity, your DDA runs inside the candidate |
| TLAS binding arrays | Added in v29 | Multiple acceleration structures indexable from one shader |
| RT backend coverage | Vulkan mature; Metal acceleration structures landed; DX12 DXR still open | Anything RT needs a non-RT fallback path regardless |
| HDR surface output | `SurfaceConfiguration::color_space` in v30 (scRGB, HDR10, HLG, Display P3) | Tone mapping now has a real HDR target, not just an LDR curve |
| 16-bit ints in WGSL | `Features::SHADER_I16` in v30 | Packed voxel/meshlet data gets cheaper |
| 64-bit atomics | **Unverified — check against your pinned version** | Gates the Nanite software-rasterizer question below |

**The other two constraints**

1. **Two geometry paths, one GBuffer.** Raymarched volumes and rasterized meshlets
   converge on shared formats. Anything that shades *during* rasterization can't
   serve both.
2. **The world is destructible.** Anything baked is invalidated by design — and any
   acceleration structure has a rebuild cost on every edit.

---

## 1. Rendering pipeline

| Technique | Verdict | Reasoning |
|---|---|---|
| Forward | Skip | Shades during rasterization; the raymarcher has no vertex stage to attach to. |
| Deferred (G-buffer) | **Take** | The only structure that lets both paths share one lighting model. Your compositor already assumes matching formats. |
| Forward+ | Skip | Same objection as forward. Its transparency advantage is also moot — water is a voxel material resolved inside the march, not a transparent draw. |
| Tiled deferred | Skip | 2D tiles degrade at depth discontinuities, which voxel terrain has everywhere: cliff edges, cave mouths, overhangs. |
| Clustered deferred | **Take** | Froxel binning is representation-agnostic, and log-depth slices are exactly right for horizon-scale range. |
| Visibility buffer | Open | Attractive for the meshlet path — thin GBuffer, attribute fetch deferred to shade time. But the raymarcher has no triangle IDs to write, so it forces the composite into two shading passes. Only worth it if the rasterizer path becomes dominant. |
| GPU-driven rendering | **Take** | Already the design. With mesh shaders now available there are two viable routes — see Geometry below. |
| Render graphs | **Take**, small | Adopt automatic barriers and transient aliasing; don't adopt UE5's scale. You have four fixed stages, not several hundred passes. A few hundred lines. A classic over-engineering sink. |

**GBuffer requirement.** Reserve a source flag distinguishing raymarched from
rasterized pixels. Both the shadow bias rule and any per-path velocity handling
depend on it, and adding a channel later touches the compositor and both renderers.

## 2. Culling & visibility

| Technique | Verdict | Reasoning |
|---|---|---|
| Frustum culling | **Take** | Stage 2, already specified. |
| BVH traversal | **Take** | `BvhAccel<T>` already spans ChunkedVolume macro structure, instances, and meshlets. |
| Hierarchical Z-buffer | **Take** | Core to Stage 2; previous-frame depth pyramid. |
| HW occlusion queries | Skip | 1–2 frame latency plus a CPU round-trip. HZB supersedes it entirely. |
| SW rasterization of occluders | Skip | UE5 needs it because the CPU must decide before submitting. Your culling is already GPU-side. |
| Contribution culling | **Take**, free | The SSE dispatch already computes projected size; a sub-pixel threshold is one comparison. |
| GPU occlusion | **Take** | This *is* the HZB compute test — same entry under a different name. |

## 3. Lighting & shadows

| Technique | Verdict | Reasoning |
|---|---|---|
| Directional lights | **Take** | Sun/moon; the primary light. |
| Point / spot lights | **Take** | Clustered culling makes the count cheap, and caves are unlit without them. |
| Shadow mapping | **Adapt** | Entity geometry only — never the world, never meshed near-field terrain. |
| Cascaded shadow maps | Skip | Cascades exist to give a rasterizer usable texel density across distance. Octree LOD already gives you that. Revisit only for large entities (below). |
| Variance shadow maps | Skip | Light bleeding, plus filtering infrastructure for a problem you won't have. |
| PCSS | **Adapt**, later | Don't do blocker search. Widen the shadow ray into a cone with distance during the march — contact hardening falls out of the traversal you already run. |
| Virtual shadow maps | Skip | Solves texel density for rasterized worlds. UE5 built it *because* it couldn't trace the world cheaply. |
| Shadow atlas | **Take**, small | Entities and local lights only. |
| Clustered light culling | **Take** | Representation-agnostic; correct regardless. |
| Area lights (LTC) | Open, later | Closed-form fit, works fine against a deferred GBuffer. A "when the art needs it" call. |

### The composite sun shadow

Evaluate the shadow term **in the deferred lighting pass, from GBuffer world
position** — not inside either renderer. Both terms then apply to every pixel
regardless of which path produced it:

```
shadow = min(svo_shadow_raymarch(p), entity_shadow_map(p))
```

| Receiver | Occluder | Covered by |
|---|---|---|
| Voxel terrain | Voxel terrain | SVO raymarch |
| Voxel terrain | Entity | Entity depth map |
| Entity | Voxel terrain | SVO raymarch |
| Entity | Entity | Entity depth map |

Wiring this per-renderer instead ("entities get maps, terrain gets rays") silently
drops row two — a character's shadow on the ground, the one people notice.

**Why the entity map is well-behaved.** It contains only entity geometry, so terrain
pixels sample a map they were never rendered into: no occluder means far depth means
lit. Terrain gets zero acne from it. Self-shadowing is confined to authored meshes
with clean normals, where normal-offset bias just works. Sizing follows: entities are
localized near the camera, so one tightly-fitted ortho frustum replaces a cascade
chain — 2048² over a 128 m box is ~6 cm texels.

**Three failure modes to design against**

1. **Never render meshed near-field terrain into the entity map.** It's the same
   geometry the SVO already describes, and Dual Contouring's isosurface is not the
   raymarched voxel surface — the two terms will disagree at grazing angles.
2. **Bias returns at the mesh/voxel seam.** Rasterized terrain shades at points up to
   half a voxel off the SVO isosurface, so shadow rays self-intersect. Offset the ray
   origin ~1 voxel along the normal when the GBuffer source flag says rasterizer.
3. **Penumbra mismatch.** A cone-widening SVO trace next to fixed-radius PCF reads as
   razor-sharp entity shadows against soft terrain shadows in the same frame. Grow the
   PCF radius with occluder distance. This is the one that looks wrong while being
   correct.

**Ray-queried entity shadows: later.** Would remove the map and give exact contact
shadows. Deferred because the feature carries a breaking-changes warning, DX12
coverage is absent so the map is needed as fallback anyway, and skinned entities need
per-frame BLAS refit — the one case where acceleration structure cost recurs every
frame rather than on edit. Swapping it in later is a branch inside a lighting pass,
not an architectural change.

**Where this breaks.** A genuinely large entity — dragon, airship — violates the
"small and near the camera" assumption the single ortho frustum rests on. If those are
in scope, they need a second cascade and the sizing argument needs redoing.

## 4. Global illumination

Godot, not UE5, is the reference here: it's the only one of the three that shipped
real-time GI without assuming hardware RT.

| Technique | Verdict | Reasoning |
|---|---|---|
| Voxel cone tracing / SDFGI | **Take** — headline | Every other implementation spends most of its budget voxelizing triangle soup into cascades on every geometry change. You have an SVO with averaged interior-node color already. Keep SDFGI's cascade structure, probe update, and ray-hit-to-irradiance path; delete the voxelization stage. |
| DDGI | **Adapt** | Take the probe placement, relight and depth-visibility scheme. Feed it SVO raymarch hits — or ray-query hits if the prototype below succeeds. |
| Irradiance volumes | **Take** | Natural storage container for traced irradiance. |
| Light probes / SH | **Take** | Low-spec fallback tier when cone tracing is off. |
| SSGI | **Take** | Cheap near-field complement — catches contact bounce that coarse SVO mips smear. |
| Ray-traced GI (HW) | **Open** — was Skip | Reopened by wgpu 30. Inline ray query plus AABB BLAS over brick bounds means RT cores traverse the coarse levels and your DDA runs inside the candidate brick. Gated entirely on acceleration structure rebuild cost — see below. |
| Radiance cascades | Open | Fast to update and a strong fit in principle, but the 3D story is still unsettled in the literature. Prototype, not commitment. |
| ReSTIR | Open, distant | Was a flat skip on the grounds that you had no ray budget. If hardware ray query lands, that objection goes away and reservoir sampling becomes relevant for many-light scenes. Still far downstream of everything else here. |
| Lightmapping | Skip | The world is destructible. Baked light is invalidated by design. |

**Emissive voxels are your cheapest light source.** Inject emissive palette entries
into the cone-trace source and lava or glowing fungus lights caves with no light list
entries at all. No equivalent in the menu.

## 5. Materials & shading

| Technique | Verdict | Reasoning |
|---|---|---|
| PBR metal-roughness | **Take** | Universal, and palette entries map onto it directly. |
| Cook-Torrance / GGX | **Take** | Standard microfacet specular. |
| Emissive | **Take** | Also a GI source — see above. |
| Disney BRDF | Skip | Ten-plus artist-facing parameters need an art team to drive them. |
| Subsurface scattering | Open, later | Entity-only: foliage, skin. Not a world-path concern. |
| Clear coat | Skip | Precipitation wetness is better done as roughness + albedo modulation off the wetness field. |
| Anisotropic | Skip | Brushed metal and hair aren't in the brief. |
| Cloth | Skip | Same. |
| Cel / toon | Skip | Unless art direction demands it — a shading-model swap, not an architecture decision. |

## 6. Post-processing

| Technique | Verdict | Reasoning |
|---|---|---|
| TAA | **Take — first** | Not post-processing. Infrastructure. The raymarcher, SSR, volumetric fog and any GI denoiser all need temporal history. |
| Tone mapping (ACES) | **Take** | Default filmic curve. |
| Tone mapping (AgX) | Open | Better hue preservation on bright emissives and god rays — you'll have a lot of both. Cheap to A/B; they're both curves. |
| HDR output | **Take** — new | wgpu 30 exposes `SurfaceColorSpace` (scRGB, HDR10, HLG) with `Surface::display_hdr_info` for headroom. Worth wiring while the tone mapper is being written rather than after; the LDR path stays as fallback where the platform doesn't support it. |
| Bloom | **Take** | Emissive voxels need it to read correctly. |
| SSR | **Take** | Water explicitly needs it; the design already names it. |
| Motion blur | **Take**, late | Shares the velocity buffer with TAA — nearly free once that exists. |
| Depth of field | **Take**, late | Standard. |
| Color grading (LUT) | **Take** | Trivial, and the cheapest art-direction lever. |
| SSAO | Skip | Redundant once cone-traced AO falls out of the GI trace. |
| GTAO / HBAO+ | Skip → fallback | Same, but keep one as the low-spec tier when GI is disabled. |

## 7. Anti-aliasing & upscaling

With a compute raymarcher, cost scales with **ray count** — dynamic resolution plus
temporal upsampling is worth far more than any edge-AA technique.

| Technique | Verdict | Reasoning |
|---|---|---|
| FSR | **Take** | Shader-only and cross-vendor — Godot's reason for standardizing on it, and the only upscaler that fits a wgpu-portable engine. |
| MSAA | Skip | Doesn't work on deferred, doesn't work on a compute raymarcher. |
| FXAA | Skip | TAA supersedes it. |
| SMAA | Skip | Same. |
| DLSS | Skip | Vendor SDK outside wgpu; needs per-backend native interop. |
| XeSS | Skip | Same interop problem. |
| MetalFX | Open | Metal-only, but you already describe a Metal path below wgpu — a cheap Apple-silicon win if you're down there anyway. |

## 8. Geometry & LOD

| Technique | Verdict | Reasoning |
|---|---|---|
| Sparse voxel octree | **Take** | Core. |
| Brickmaps | **Take** | Core. 16³ bricks with occupancy masks. |
| Nanite | **Adapt** | Cluster hierarchy, GPU SSE, indirect draw: yes. The software rasterizer depends on 64-bit atomic visbuffer writes — verify availability on your pinned version before assuming either way. |
| Mesh shaders | **Open** — was Skip | Available since wgpu 28. Full on Vulkan, passthrough shaders on Metal and DX12. The trade is real: they're the natural fit for meshlet rendering, but passthrough means maintaining backend-specific shader code against a cross-platform premise. Compute + `draw_indexed_indirect` remains the portable route and is what DESIGN.md specifies. Prototype both against your meshlet format before committing. |
| Multi-draw indirect | **Take** | Backbone of the rasterizer path, and the fallback if mesh shaders don't win. |
| HW instancing | **Take** | FlatGrid props — trees, rocks, pebbles — are exactly this case. |
| Discrete LOD | Skip → meshlets | Irrelevant for the world (the octree is continuous); survives only as meshlet DAG levels. |
| SVO-DAG | Open, later | The 10–100× memory claim is real, but subtree dedup makes writes expensive and fights runtime editing. A shipped-world compression format, not the live structure. |
| Signed distance fields | Open | Not a replacement for bricks. Useful as a secondary representation for smooth CSG authoring, and as cheap acceleration for soft shadow cones. |
| Heightmap terrain | Skip | You have volumes. |
| Clipmap terrain | Skip | The world skeleton's residency ring is the same idea at cell granularity. |

## 9. Streaming & memory

| Technique | Verdict | Reasoning |
|---|---|---|
| Geometry streaming | **Take** | Core; already the OOC pipeline. |
| Upload budgeting | **Take** | Core; hard VRAM/RAM caps. |
| Always-resident LOD | **Take** | UE5's root-cluster policy generalizes cleanly: pin the coarsest SVO mip so no frame has nothing to draw. |
| Ring buffer staging | **Take**, flagged | Correct design — see the risk note. |
| GPU buffer pooling | **Take** | Suballocate the brick pool rather than allocating per brick. |
| Level streaming | **Adapt** | The world skeleton residency states already are this, at finer granularity. |
| Virtual texturing | Skip | Solves UV-space texture streaming. You have no UVs — the per-brick palette replaces it. |

**Risk.** DESIGN.md describes Metal blit encoders and Vulkan async transfer queues,
but wgpu doesn't expose a separate transfer queue — you'll be in `wgpu-hal` or
accepting `write_buffer`'s internal staging. The never-stall claim rests on this.

## 10. Scene architecture

| Technique | Verdict | Reasoning |
|---|---|---|
| SlotMap storage | **Take** | Already benchmarked (sw-cf6350). |
| Spatial BVH | **Take** | Shared by culling, queries and instances. |
| Octree | **Take** | It *is* the world. |
| Dirty tracking | **Take** | Per-object flags; universal for a reason. |
| World partitioning | **Take** | The world skeleton is this. |
| Material system | **Take** | Palette indices are your material handles. |
| ECS (archetype) | Defer | As designed. Archetype migration measured 500–660× slower than field mutation. |
| Scene graphs | Skip | Entities are flat with side structs. The only genuine trees are skeletal bone hierarchies. |

## 11. Threading & jobs

| Technique | Verdict | Reasoning |
|---|---|---|
| Work-stealing | **Take** | Chase-Lev deques; already the design. |
| Atomic prerequisites | **Take** | Decentralized DAG resolution — what your dependency graph needs. |
| Parallel-for | **Take** | Range-split for meshing and worldgen batches. |
| Frame pipeline | **Take — decide now** | Game(N+1) \| Render(N) \| GPU(N−1). Forces double-buffered render state throughout; brutal to retrofit. Your Cull stage already reads previous-frame depth. |
| Named threads | **Take** | Audio has a hard realtime deadline and must never sit on a stealing pool. Render submission likewise. |
| Priority + promotion | **Take** | You already have frame-critical vs background categories — exactly the starvation problem promotion solves. |
| Serial pipes | **Take** | Cheap, and the right shape for ordered per-subsystem work without dedicating a thread to each. |
| Task retraction | Skip, initially | A fix for one pathology: a worker blocking on a task still sitting in a queue. Add it when you hit that deadlock. |
| FIFO pool | Skip | Godot's honest simple answer, but you've committed to work-stealing. No reason to run two models. |

## 12. Physics & animation

Entities are meshed and rigged, so this column is in scope. Split by what collides
with what.

| Technique | Verdict | Reasoning |
|---|---|---|
| Raycasting | **Take** | Octree DDA already exists; it *is* your raycast. |
| Skeletal animation | **Take** | Entities are rigged by design. |
| Blend trees | **Take** | Standard, and consensus across all three for good reason. |
| Rigid body | **Take**, later | Needed for destruction debris if nothing else. |
| GJK / EPA | **Take**, entity-vs-entity only | Convex narrowphase for characters and props. Entity-vs-world is swept AABB against the grid. |
| IK | **Take**, later | Foot placement on voxel terrain is precisely where IK earns its cost. |
| Ragdoll | **Take**, later | Falls out of rigid body plus the skeleton. |
| Sweep & prune | Skip → BVH | The brick grid is already the broadphase for entity-vs-world; `BvhAccel` covers entity-vs-entity. |
| Procedural animation | Open | Art-direction dependent. |

---

## Decide now, before it gets expensive

**1. The velocity buffer.** TAA requires motion vectors from *both* paths in the
shared GBuffer. Camera-only reprojection covers the static world; moving entities
break without per-path velocity. Cheap to add now, expensive later — it touches the
compositor, both renderers, and every temporal consumer.

**2. Frame pipelining depth.** Three-deep forces double-buffered render state
everywhere. Decide before the render state layout hardens, even if you implement later.

**3. Acceleration structure rebuild cost.** *The* prototype. wgpu 30's AABB BLAS makes
hardware-accelerated traversal of your own brick structure possible, which would reopen
RT-based GI and entity shadows. But every brick edit invalidates a BLAS and every
streamed region needs one built — TLAS refit is cheap, BLAS rebuild isn't. No engine in
the menu can answer this, because none of them mutate and stream geometry at your rate.
Measure: build time per brick edit and per streamed chunk, against the budget
controller's frame allowance. If it fits, several Skips above become Takes. If not,
software SVO traversal stands unchanged.

**4. Mesh shaders vs compute + indirect draw.** Now a live fork rather than a
non-option. Decide against your meshlet format, weighing passthrough-shader
maintenance on Metal and DX12.

**5. meshoptimizer vs own clustering.** Left open in DESIGN.md, and it gates the
meshlet DAG format, the SSE pass, and the indirect draw layout.

**6. wgpu transfer queue exposure.** See the streaming note.

## Candidates the menu is missing

Assembled from three triangle-first engines, so it has blind spots exactly where you
operate:

- **ESVO / efficient sparse voxel octree traversal** — the Laine & Karras line of work
  is your actual reference for the world path; no engine on the list implements it.
- **AABB-BLAS brick traversal** — hardware BVH down to brick bounds, software DDA
  inside. Only possible as of wgpu 30, and in no engine's playbook.
- **Temporal reprojection of ray results** — reusing last frame's march at coarse LOD.
  The raymarcher analogue of TAA, and a much bigger win than edge AA.
- **Blue-noise / stochastic sampling with temporal accumulation** — the denoising
  substrate for everything you trace.
- **Checkerboard or variable-rate marching** — half-rate rays for distant or
  low-variance regions.
- **Transvoxel** — in DESIGN.md for chunk stitching, absent from the menu.
- **Dual contouring QEF placement** — likewise.
- **Cone-traced ambient occlusion** — falls out of the GI trace, replaces SSAO.
- **Hierarchical emissive injection** — propagating emissive voxels up SVO mips so
  glowing geometry lights scenes without light-list entries.
