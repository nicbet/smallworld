# Architecture Design Review — `docs/architecture-design.md`

**Date:** 2026-08-11 (revised after positioning discussion; disposition added after the same-day
architecture update)
**Scope:** Internal consistency, technical validity, design flaws, adherence to `notes/research/ue5/architecture.md`, gaps/omissions.
**Status:** **CLOSED** (2026-08-11). Every finding is dispositioned — resolved in
`architecture-design.md` or promoted to its Open Questions (see Disposition section). This
document is a durable record of the review round; ongoing work happens against the Open
Questions in `docs/architecture-design.md`. The findings below (§§1–8) describe the architecture
**as reviewed on 2026-08-11**, kept intact as the record of that discussion.
**Positioning baseline:** smallworld is "UE5 in Rust, minus visual Blueprints." Voxel worlds are
supported out-of-the-box via a first-class **Voxel Plugin** built on the public plugin API. Voxel
findings below are therefore framed as plugin-API sufficiency tests, never as engine-core
motivations.

---

## Disposition — after the 2026-08-11 architecture update

### Resolved in `architecture-design.md`

| Finding | Resolution |
|---------|-----------|
| §1.1 `GeometryBackend` straddles the firewall | Split into `GeometryExtractor` (game side, no GPU types) + `GeometryRenderer` (render side, owns retained data, may use wgpu); registered as a pair |
| §1.2 Snapshot vs. retained contradiction | Retained `RenderScene` embraced explicitly; `FramePacket` is now a delta message; `STATIC` caching is real |
| §1.3 HZB from mesh-only pre-pass | HZB built after the GBuffer/VolumePass stage from final opaque depth (all backends occlude) |
| §1.4 Missing common geometry currency | Both contracts: shared mesh stream (`SceneDeltaWriter` mesh lane) + participation traits (`ShadowCaster`, `TlasContributor`) |
| §2.2 RT assumes hit shaders / SBT | RT section rewritten `ray_query`-only; manual hit-point fetch; SVO **compute** raymarcher named as the Voxel Plugin's shadows/GI path |
| §2.3 TLAS ordering vs. culling | TLAS built pre-cull from the retained scene, under a dedicated RT culling radius |
| §2.4 Feedback ignores GPU asynchrony | Frames-in-flight readback ring; every GPU-derived datum stamped with the frame it measures; render thread never blocks |
| §2.6 Custom materials can't do toon | Per-pixel 4-bit Shading Model ID in albedo alpha; `ShadingModel` on `CustomMaterial`; lighting pass switches per pixel |
| §3.1 Voxel Plugin can't cast shadows | Shared stream (`CAST_SHADOW`) + `ShadowCaster` participants feed per-shadow-view draw lists |
| §3.2 GI fallback framing | SSAO+probe documented as a conscious v1 cliff; lighting GI/shadow-mask inputs are a public contract plugins can feed |
| §3.3 No instancing, HashMap pools | `MeshDrawCommand.instances: Range<u32>` + shared `InstanceData` buffer; pools are dense generational `SlotMap`s |
| §3.4 Single-camera core types | `ViewParams`/`ViewKind` (Main/Aux/Shadow); per-view culling; shadow views derived render-side |
| §3.5 No Camera component | `Camera` component added (projection, target, priority); active cameras become views |
| §4.1 `DrawProcessor` mesh-only | Reframed as intentional and documented: it operates on the shared mesh stream from all backends |
| §4.2–4.7, §4.9, §4.10 | Fixed as specified (GameContext methods documented, `assets: &mut`, AS deduped, `history_shadow`, mask-channel wording, `mip_count`, `PassId`, throughput wording) |
| §7 ChangeTracker vs. resources | `dirty_resources` added; `ResourceHandle` mutations tracked |

### Deferred into the design doc's Open Questions (decision pending, follow-up scheduled)

| Finding | Open Question |
|---------|---------------|
| §2.1 Volume depth writes | OQ 1 — *note: volume motion vectors for raymarched detail should ride along in this discussion; extracted tiers already get velocity via `InstanceData`* |
| §2.5 WGSL has no optional bindings | OQ 2 (permutations vs. dummy bindings) |
| §2.2 residual — surface cache / bindless hit fetch | OQ 3 |
| §3.2 residual / §6.3 — core software-GI tier | OQ 4 |
| §5.2 Asset payload deep copies | OQ 5 |
| §5.1 script world access, §5.3 event semantics, §5.4 fixed-step interpolation | OQ 6 |

### Promoted to Open Questions (second batch, same day)

Everything that remained standing was added to the design doc's Open Questions as OQ 7–23.
**Nothing from this review round is left without a disposition** — every finding is either
resolved in `architecture-design.md` or tracked as a numbered open question there.

| Finding | Open Question |
|---------|---------------|
| §6 GPU-driven rendering stance | OQ 7 |
| §6 Frame pacing / latency control | OQ 8 |
| §6 Translucency lighting + volumetrics | OQ 9 |
| §7 Seamless LOD transitions | OQ 10 |
| §7 IBL / reflection probes (`EnvironmentParams`) | OQ 11 |
| §7 Auto-exposure + temporal upscaling | OQ 12 |
| §7 Decals | OQ 13 |
| §7 Skinning design | OQ 14 |
| §7 Resize / device-lost / teardown ordering | OQ 15 |
| §7 Physics architecture + worker-pool priorities | OQ 16 |
| §7 Streaming section (World Partition analog) | OQ 17 |
| §7 UI stance | OQ 18 |
| §7 Networking stance | OQ 19 |
| §7 Save / serialization | OQ 20 |
| §4.8 `VolumeSource` in component / `brick_residency` ownership | OQ 21 |
| §4.11 GBuffer ID row vs. implemented contract (sw-6dd982) | OQ 22 |
| §4.12 CLAUDE.md entity-model text vs. this doc | OQ 23 |

(Volume motion vectors, previously noted as a rider, are now explicitly folded into OQ 1.)

---

## Verdict

The document is strong where it adapts UE5's ideas deliberately: the Game/Render thread split, the
hybrid RT contract (raster writes GBuffer, RT reads it), the five-level customization ladder, and the
ownership-boundary table are all faithful, well-reasoned translations into Rust. The honest framing of
feedback as advisory and capability-gated RT as purely additive are good calls. (Dropping
`FMeshBatch` simplifies the mesh path, but it carries an unacknowledged structural cost — §1.4.)

However, there are **four structural problems** that undermine the document's own stated
principles, **a cluster of wgpu-reality problems** in the RT and volume sections, and **a set of
pass-participation gaps that the doc's own dogfooding principle turns into API verdicts**: the
shipped Voxel Plugin cannot, through the public contracts as written, cast shadows, occlude other
geometry, or write depth (§1.4, §3). "If the API isn't powerful enough for voxels, it isn't
powerful enough for games" (`:314`) — by that yardstick the current API fails its own test. The
biggest strategic question the doc leaves unresolved:
**snapshot-per-frame vs. retained render scene** — it claims the former while quietly depending on
the latter.

---

## 1. Critical structural findings

### 1.1 `GeometryBackend` straddles the thread firewall it's supposed to enforce

`docs/architecture-design.md:320-348`

The trait mixes methods that must run on different threads against different data:

- `extract(&self, world: &World, ...)` — must run on the **Game Thread** (borrows `&World`).
- `cull(&self, ..., hzb: Option<&wgpu::TextureView>)` — must run on the **Render Thread**
  (takes a GPU texture view; §2 of the pipeline says culling is a Render Thread phase).
- `prepare(&mut self, ..., state: &mut RenderState)` — **Render Thread**, and takes `&mut self`.

So who owns the backend object? If the Render Thread owns it, the Game Thread can't call
`extract`. If it's shared (`Arc<dyn GeometryBackend>`), `prepare(&mut self)` is impossible
without a lock — exactly the shared mutable state Principle 2 forbids. As specified, this trait
cannot be implemented in safe Rust without violating the document's own thread-ownership model.

UE5 solves this precisely with the split the doc chose to drop: `UPrimitiveComponent` (game side)
vs. `FPrimitiveSceneProxy` (render side). **Recommendation:** split the trait into a
game-side `GeometryExtractor` and a render-side `GeometryRenderer`, registered as a pair. The
`hzb` parameter also leaks a raw `wgpu::TextureView` into an API games implement — a direct
violation of Principle 3 ("Game code never sees a `wgpu::Device`, a bind group, or a GPU buffer").

### 1.2 Snapshot vs. retained scene — the central unresolved contradiction

`docs/architecture-design.md:456-481`, `:527`

The doc claims: *"Instead of maintaining persistent mirror objects on the Render Thread, we send a
self-contained `FramePacket` each frame"* and *"No references, no `Arc`... Once sent, the Game
Thread is free to mutate the World."*

Three lines later: *"Change-driven. ... Unchanged entities reuse their draw commands from the
previous packet."*

These cannot both be true. Either:

- **(a)** Each packet contains all draw commands (the "reuse" is a game-side cache that clones into
  every packet) — then at battlemoon scale (tens of thousands of instances) you memcpy the entire
  draw list every frame, and the `FMeshDrawCommand`-style caching claim at `:527` ("can be sorted,
  merged, and cached") is hollow — nothing survives across frames on the render side to cache
  against; or
- **(b)** The Render Thread retains previous draw sets and patches them from deltas — which **is** a
  persistent mirror scene, i.e., the proxy model the doc says it rejected.

The `EntityFlags::STATIC` hint (`:850`, "enables caching") promises optimization the snapshot model
cannot deliver. **Recommendation:** embrace (b) explicitly. A retained `RenderScene` updated by
spawn/despawn/transform deltas is the actual load-bearing insight of UE5's proxy system, and it's
what the change tracker is already shaped for. The doc should own that decision rather than
describe (a) while depending on (b).

### 1.3 HZB is built from the mesh-only depth pre-pass — plugin geometry never occludes anything

`docs/architecture-design.md:141-145`, `:363-372`

HZB construction is placed in the Depth Pre-Pass, which renders **opaque meshes** only
(`MeshBackend` registers `DepthPrepass`; `VolumeBackend` registers only `VolumePass`, which runs in
the GBuffer stage). Consequence: the occlusion HZB contains only `MeshBackend` geometry. Any
non-mesh backend — the shipped Voxel Plugin first among them — contributes nothing to the HZB, so
its geometry can never occlude anything. In a voxel-terrain game (the plugin's headline use case),
the largest occluder class is invisible to occlusion culling: a mountain of voxels culls nothing,
while a triangle pebble does. The same failure applies to any game-defined backend (dense
particles, procedural terrain, SDF shapes).

**Recommendation:** build the HZB after the GBuffer/VolumePass stage from the final opaque depth
(this is what UE5 effectively does — its HZB is fed by the full depth after the base pass). That
fixes coverage for every backend at once — provided backends can write depth at all (§2.1).

### 1.4 Dropping `FMeshBatch` silently removed the common geometry currency — engine passes can't consume plugin geometry

`docs/architecture-design.md:351-360`, `:363-372`, `:527`

In UE5, every proxy — including custom ones — expresses its geometry as `FMeshBatch`, and every
engine pass (depth, shadow, base, velocity) consumes `FMeshBatch` via its `FMeshPassProcessor`
(`ue5/architecture.md:158-160`, `:185-189`). That shared currency is *why* custom geometry in UE5
gets shadows, depth, and occlusion "for free."

Smallworld dropped the intermediate layer: each backend's `DrawCommandSet` is opaque
(`Any`-downcast, `:351-360`), consumable only by passes that know its concrete type — i.e., the
backend's *own* passes. Engine-owned passes (`DepthPrepass`, `ShadowPass`, and the implied
velocity/TLAS producers) only understand `MeshDrawCommand`. The structural consequence: **a plugin
can add its own passes, but it can never participate in the engine's passes.** That single fact
produces the shadow gap (§3.1), the HZB gap (§1.3), the TLAS question (§2.3), and the volume
motion-vector gap (§7) — they are one design hole, not four separate bugs. For an engine whose
strategy is "ship the Voxel Plugin on the public API," this is the load-bearing finding.

**Recommendation — two contracts, probably both:**

1. **Mesh interop:** allow any backend to *also* emit standard `MeshDrawCommand`s into the shared
   mesh stream (e.g., the Voxel Plugin's extracted-mesh LOD tiers). Those draws then participate
   in depth/shadow/HZB/TLAS/velocity automatically — the cheap 90% answer, and exactly how UE5
   custom proxies behave.
2. **Pass-participation traits** for genuinely non-triangle geometry: e.g., `ShadowCaster`
   (render depth into a given shadow view), `DepthContributor` (prepass/HZB), `TlasContributor`.
   Engine passes iterate registered participants instead of hardcoding mesh commands. This is what
   makes the plugin API actually sufficient by the doc's own `:314` standard.

---

## 2. Technical validity issues (wgpu/GPU realities)

### 2.1 Volume depth writing is hand-waved, and the stated mechanism can't work

`docs/architecture-design.md:161-166`

*"Raymarched volumes write via compute shaders through the `VolumePass`, reading depth to composite
correctly."* Two problems:

1. **Compute shaders cannot write a `D32Float` depth attachment.** Depth writes happen only via
   raster (`frag_depth` or fixed-function). A compute VolumePass can at best write depth-as-data to
   an `R32Float` storage texture, which then must be merged into the real depth buffer somehow.
2. **`Rgba8UnormSrgb` cannot be a storage texture** in WebGPU/wgpu — the albedo target as specified
   cannot be written from compute at all.

And volume depth is not optional: transparency sorting against volumes, TAA reprojection, RT ray
origin reconstruction ("position reconstructed from depth", `:226`), motion vectors, and next-frame
HZB all consume the depth buffer. If volumes aren't in it, every one of those systems is wrong
wherever a volume is on screen. Options worth discussing: raster proxy geometry with
`frag_depth` export from a fragment-shader raymarcher; or compute writes `R32Float` + a
depth-merge raster pass; or non-sRGB albedo with manual encode. This is a known wgpu-friction
area — worth adding to the friction log either way.

### 2.2 The RT sections assume shader-binding-table features wgpu does not have

`docs/architecture-design.md:216`, `:232-234`

- *"SVO traversal in ray-any-hit... requires `ray_tracing_pipeline` support"* and *"Hit points
  sample their own material (via the BLAS hit shader...)"* — wgpu exposes **inline ray queries
  only** (experimental features), usable from compute/fragment. There is no ray tracing pipeline,
  no SBT, no hit/any-hit/intersection shaders, and no public roadmap commitment to add them. The
  "long-term path" as written depends on a feature that may never exist in the chosen API.
- With `ray_query`, a hit gives you instance/primitive indices — the shader must then **manually
  fetch** vertex attributes and material data, which requires bindless-style access to all vertex
  buffers and materials, or a surface cache. The doc name-drops "surface cache" without designing
  it; it's actually the *mandatory* piece, not the alternative.
- **Platform coverage:** wgpu ray-query support has landed for Vulkan/DX12; Metal coverage is
  questionable. Primary development happens on macOS — the RT path may be untestable on the dev
  machine. Worth verifying against current wgpu before committing to this design. (A compute-shader
  SVO raymarcher for volume shadows/GI needs none of this — see §3.1.)

### 2.3 TLAS-from-draw-commands: ordering vs. culling is unspecified and matters

`docs/architecture-design.md:195`, `:211`

*"Rebuilt every frame from the `FramePacket`'s draw commands — each `MeshDrawCommand` becomes a TLAS
instance entry."* But `cull()` filters the draw command sets in place on the Render Thread. If the
TLAS is built after culling, everything off-screen vanishes from reflections, shadows, and GI —
defeating the entire point of RT secondary effects (a mirror showing the room behind the camera).
If it's built before culling, the doc should say so, and note that RT needs a *different, larger*
culling domain than raster (UE5 has a separate ray-tracing culling radius for exactly this reason).
This interacts with extraction too: if `extract()` is camera-aware (it takes `CameraParams`),
distance-culled extraction starves the TLAS before culling even runs.

### 2.4 FrameFeedback timing ignores GPU asynchrony

`docs/architecture-design.md:54-104`, `:1303`

`FrameFeedback` is sent at PRESENT and carries `gpu_time` (timestamp queries) and
`readback` (occlusion queries, compute results). But at the point `present()` is called, the GPU
has not *executed* the frame — timestamps and query results aren't available. In wgpu, readback
requires `map_async` + polling, which resolves one-to-several frames later. So either the Render
Thread blocks on the GPU each frame (destroying the pipelining the whole architecture exists to
provide), or the feedback's GPU data is *older than the packet it rides in* — i.e., the "N-2"
labeling is wrong for GPU-derived fields. **Recommendation:** an explicit frames-in-flight readback
ring (2–3 buffered query sets), with per-field frame indices in `FrameFeedback` so consumers know
the actual age of each datum. Also note: the N-2 relationship is a *typical* case, not a guarantee —
it depends on channel timing; the doc states it as invariant.

### 2.5 "The shader branches on whether RT targets are bound" — WGSL can't do that

`docs/architecture-design.md:260`

There are no optional bindings in WGSL/WebGPU. A bind group layout is fixed at pipeline creation.
The real mechanism is pipeline permutations (shader defs / preprocessor) or always-bound dummy
textures + a uniform flag. Minor, but it's the difference between "the render graph handles this
naturally" (`:287`) and a shader-variant system that must be designed and built.

### 2.6 Custom materials cannot deliver the examples the doc promises

`docs/architecture-design.md:431-450`

Toon shading and stylized water are named as goals, but the composition hooks only let a material
control *what gets written to the GBuffer*. The lighting pass is fixed Cook-Torrance
(`:176-182`). Toon shading is a custom *lighting response* — you cannot express it by writing
different roughness values. UE5 handles this with a per-pixel **Shading Model ID** in the GBuffer
that the lighting shader switches on; smallworld's GBuffer has flag bits in albedo alpha
(`:154`) but no shading-model concept. Either add a shading-model ID to the GBuffer contract (the
recent GBuffer contract work already touches this layout) or scope custom materials honestly to
"custom PBR input computation."

---

## 3. Plugin-API sufficiency findings (the doc's own dogfooding test)

The doc's stated yardstick (`:314`): the engine's voxel support is "a geometry backend plugin, not
a special case… If the API isn't powerful enough for voxels, it isn't powerful enough for games."
Under the "UE5 in Rust + shipped Voxel Plugin" positioning, that sentence *is* the product
strategy — so each finding below is an engine-API verdict demonstrated by the Voxel Plugin, and
applies equally to any game-defined backend.

### 3.1 The shipped Voxel Plugin cannot cast shadows

`docs/architecture-design.md:363-372` (backend/pass table), `:168-174`

`VolumeBackend` registers only `VolumePass`. The shadow atlas is populated by `ShadowPass`, which
only `MeshBackend` registers. Result: **volume geometry casts no shadows** — a voxel hill shadows
nothing; a mesh crate next to it does — and nothing in the doc acknowledges it. This is §1.4 made
concrete: no non-mesh backend (particles, procedural terrain, SDF shapes) can cast shadows through
the public API. The fix is §1.4's participation contracts plus §3.4's shadow views; given those,
the Voxel Plugin has three implementation options — extracted-mesh proxies into the atlas (cheap,
reuses its existing extraction strategy), raymarched depth into atlas tiles (subject to §2.1's
depth-write problem), or plugin-provided SVO shadow rays at lighting time (needs no wgpu RT
features at all).

### 3.2 The engine's software-GI fallback can't lean on voxels — core needs its own stance

`docs/architecture-design.md:277-287`

The no-RT fallback tier is "SSAO + ambient probe" — a far bigger quality cliff than UE5's, whose
Lumen degrades through a full *software* ray-tracing path (distance fields,
`ue5/architecture.md:78`) before giving up. The tempting fix — SVO-traced GI — is off the table
for engine *core* under the plugin framing: core cannot assume a voxel structure exists in every
game. The real decision: accept the SSAO+probe cliff for v1 and say so explicitly, or commit core
to a general software tier (an SDF scene à la Lumen, or screen-space GI). Voxel-cone-traced GI
remains very attractive — as a *Voxel Plugin feature* for voxel-heavy games. That in turn means
the lighting pass's GI / shadow-mask inputs (`:250-260`) must be a **public contract that plugin
passes can feed**, not a private arrangement between engine RT passes and the lighting shader.

### 3.3 No instancing, no GPU-driven path — at odds with UE5-class scale

`docs/architecture-design.md:513-527`, `:131-138`, `:618-656`

- `MeshDrawCommand` has no instance count, no per-instance data reference. One command per object.
  The *current* engine's core is `SlotMap<EntityId, Instance>` — instanced objects — and the
  new architecture's draw path drops instancing entirely.
- Culling is CPU-side (worker-pool frustum tests per command). "Sort & batch" is asserted but with
  per-mesh `HashMap<GpuId, GpuMesh>` buffers there's nothing to merge into — no vertex pooling, no
  multi-draw, no indirect draws, no bindless. UE5's answer at scale is GPU Scene + auto-instancing
  + GPU-driven culling (Nanite being the extreme). An engine positioned as "UE5 in Rust" neither
  adopts nor consciously rejects GPU-driven rendering — it's simply absent. At open-world scale
  (battlemoon, via the Voxel Plugin, being the first customer) per-object CPU commands will be the
  frame budget.
- `HashMap` for the hot per-draw lookup path also contradicts the project's performance-first rule;
  `GpuId` should index a dense pool (`SlotMap`/`Vec` + generation), same pattern as everywhere else.

**Recommendation:** even if GPU-driven rendering is phase 2, the *contracts* should not preclude it:
`MeshDrawCommand` needs instancing now, and the render graph should assume indirect-draw-shaped
buffers exist eventually.

### 3.4 Single-camera assumption is baked into the core types

`docs/architecture-design.md:327-341`, `:467`

`extract()` and `cull()` take one `CameraParams`; `FramePacket` holds one camera. Consequences:

- **Shadow views are unaccounted for.** Shadow casting needs per-light/per-cascade culling and draw
  lists — a directional light's cascades see geometry the main camera culled. The doc's culling
  section covers only the main view; UE5's InitViews explicitly includes shadow setup
  (`ue5/architecture.md:46`). As specified, the shadow atlas would be fed by main-camera-culled
  commands — wrong.
- No split-screen, no render-to-texture (security cameras, mirrors, portals), no reflection
  probes, no editor viewports. UE5's `FSceneView`/view-family model exists for this.

**Recommendation:** `FramePacket` carries a `Vec<ViewParams>` (main + shadow + aux views), and
culling is per-view. This is much cheaper to design in now than to retrofit.

### 3.5 There is no Camera in the component model

The Core Engine Components (`:857-950`) define transforms, renderers, lights, materials — but no
camera component. `CameraParams` appears fully formed in the packet. Who computes view/projection,
where does a game put its camera, how do you have two? Small omission, but it's the one component
every game touches first.

---

## 4. Internal consistency issues (doc vs. itself)

| # | Issue | Location |
|---|-------|----------|
| 4.1 | `DrawProcessor` filter/sort/bind take `&MeshDrawCommand` — the extensibility story is backend-agnostic `DrawCommandSet`s, but the customization layer only works for meshes. Volume/particle/custom backends can't use it. | `:398-419` |
| 4.2 | `GameContext` struct (`:1001-1009`) has fields only (`world`, `input`, ...) — but earlier sections call `ctx.feedback()`, `ctx.register_geometry_backend()`, `ctx.register_system()`, `ctx.set_draw_processor()`. Where do the registries and feedback live? The two presentations don't reconcile. | `:97-102`, `:379`, `:416`, `:1057` |
| 4.3 | `GameContext.assets: &'a AssetServer` is immutable, but `AssetServer::load(&mut self, ...)` — games cannot load assets through the API as written. | `:1005`, `:1121` |
| 4.4 | `AccelerationStructure` is defined twice with different fields (`tlas_dirty` present at `:198`, absent at `:606`). Duplication drift. | `:197-209`, `:605-613` |
| 4.5 | `RTShadowPass` output is "denoised temporally" but `RTTargets` has history buffers only for GI and reflections — no `history_shadow`. | `:228`, `:266-274` |
| 4.6 | RT shadow mask: "one channel per shadow-casting light (up to 4 **per tile**)" — channels are per-texture, not per-tile; a per-tile light→channel mapping is a real (unspecified) sub-system. | `:227` |
| 4.7 | `ResourceOp::UploadTexture` carries no mip data, but `TextureAsset.mips: bool` exists. Who generates mips, and where do they cross the boundary? | `:504`, `:1168-1174` |
| 4.8 | "Components are plain data structs" (Principle 1) vs. `VolumeRenderer.source: Box<dyn VolumeSource>` — a trait object with behavior inside a component. Also unanswered: `VolumeDrawCommand.brick_residency` is produced at extract (Game Thread) but residency is streaming/GPU state — which side owns it? | `:897-908`, `:529-538` |
| 4.9 | `FrameFeedback.pass_timings: Vec<(String, f32)>` — per-frame `String` allocation churn in a perf-telemetry struct; should be interned IDs. | `:79` |
| 4.10 | "Doubled throughput" from pipelining is only true when both threads are equally loaded; as stated it over-promises. | `:30` |
| 4.11 | GBuffer table says Entity ID is "Entity/material ID" — ambiguous which; the recent GBuffer contract work (velocity + source flag + material ID) suggests the doc has drifted from the implemented contract. The doc also claims downstream passes "never know which path produced a pixel" while the implemented contract carries a source flag — presumably because downstream *does* need it. | `:158`, `:166` |
| 4.12 | The doc's entity model ("dense, cache-friendly arrays" per component type, `:747`) is an ECS in all but name, while `CLAUDE.md` still says SlotMap + side structs with ECS deferred. Given the 2026-08-09 decision that ECS is the standard, `CLAUDE.md` and this doc should be reconciled — right now three sources of truth disagree in flavor. | `:745-758` |

---

## 5. Rust-level validity problems in the gameplay layer

### 5.1 Scripts and the `&mut World` aliasing wall

`docs/architecture-design.md:782-795`

`ScriptInstance::update(&mut self, entity, ctx: &mut ScriptContext)` where `ScriptContext.world:
&mut World` — but scripts *are components stored in the World*. Iterating the script components
while handing each one `&mut World` is a textbook borrow conflict; the same applies to `System::run
(&mut self, world: &mut World)` if systems are stored in the World (they appear to be registered
into the engine, so systems are fine — scripts are not). Standard solutions: move script storage
out of the World, or give scripts a command-buffer API (`ScriptCommands`) applied after iteration,
mirroring the `AudioCommands` pattern the doc already uses. Needs an explicit decision, because it
shapes the whole scripting API surface.

### 5.2 The "no Arc" rule forces pathological copies

`docs/architecture-design.md:477`, `:497-508`, `:1313`

`ResourceOp::UploadMesh/UploadTexture` carry `Vec<Vertex>`/`Vec<u8>` by value, and the ownership
table confirms assets are "**Copied** into ResourceOp at extract." A 4K texture or a large mesh
means the Game Thread memcpys tens of MB inside the extract step — a hitch, per asset, on the
latency-critical thread. An `Arc<[u8]>` of *immutable* asset bytes crossing a channel is exactly as
thread-safe as an owned Vec (Principle 2's real target is shared *mutable* state). The dogma is
stricter than the principle requires and directly costs frame time. Alternative: hand off
pre-populated staging buffers.

### 5.3 Event bus semantics are underspecified

`docs/architecture-design.md:1243-1254`

"Events live for one frame and are cleared automatically" — cleared *when*, relative to system
phases? If system A (Update) sends and system B (PreUpdate) reads, does B see it this frame or
never? The standard answer is double-buffering (readable for the *next* frame), but the doc doesn't
say, and this is a classic source of heisenbugs. Same question for `fixed_update` — do events
sent in a fixed tick survive to the variable-rate update?

### 5.4 Fixed timestep with no interpolation story

`docs/architecture-design.md:993`

`fixed_update` at 60 Hz + variable-rate rendering = visible judder for physics-driven motion unless
transforms are interpolated (or extrapolated) between fixed steps at extract time. The doc has
`prev_matrix` for motion vectors but no fixed-step interpolation. UE5 hides this inside its physics
sub-stepping; a from-scratch engine must decide explicitly: interpolate (adds one fixed-tick of
latency), extrapolate (mispredicts), or lock render to fixed rate.

---

## 6. UE5 adherence scorecard

Against `notes/research/ue5/architecture.md`:

**Faithfully adopted (good):**

- Game/Render thread split with owned-data handoff — the Rust translation (channels + owned
  packets vs. UE5's command queue + proxies) is idiomatic and defensible.
- Hybrid RT contract — "raster writes the GBuffer, RT reads it, RT outputs land in dedicated
  targets composited by lighting" mirrors the research doc (`ue5:105-138`) exactly, including
  additive-only integration.
- The customization ladder maps 1:1 and honestly: `FPrimitiveSceneProxy`→`GeometryBackend`,
  `FMeshPassProcessor`→`DrawProcessor`, `ISceneViewExtension`→`add_pass`, RDG→`RenderGraph`,
  material graph→shader composition. The "voxels are a plugin, not a special case" framing is the
  best idea in the document.
- Composability principles: components-as-data, Data-Only-Blueprint→asset descriptors,
  GAS-tags→GameplayTags are all correctly identified as the transplantable ideas (the research
  doc's own conclusion, `ue5:253-289`).

**Deliberate, justified divergences:**

- 2-frame vs. 3-frame pipeline (dropping the RHI thread) — sound, wgpu owns API translation. One
  caveat: UE5's RHI thread also offloads command *submission* cost; smallworld's Render Thread
  absorbs encode+submit+present, so it will bottleneck earlier than UE5's render thread does.
  Acceptable, but worth knowing.
- No `FMeshBatch` intermediate — a good simplification *for the mesh path*; but see §1.4 for the
  cross-backend cost the doc doesn't acknowledge. `DrawProcessor` preserves the per-pass
  customization purpose (for meshes only — §4.1).
- No GameMode/GameState framework — reasonable for a code-first engine.

**UE5 ideas the doc drops without engaging them (should be conscious decisions, not silences):**

1. **Retained scene** — see §1.2. The doc frames proxies as C++ baggage, but the retained scene is
   what makes UE5's static-draw caching and incremental updates work.
2. **GPU-driven rendering** — the research doc's Nanite sections (`ue5:45`, `:53`, `:174`) are the
   single biggest UE5 idea of the last decade (move culling/LOD to the GPU, visibility buffer,
   micro-cluster streaming). Smallworld adopts none of it and doesn't say why. For an engine
   positioned as UE5-in-Rust this needs to be a conscious decision — and the Voxel Plugin would be
   its first beneficiary ("Nanite-for-bricks": GPU brick culling, per-brick LOD on GPU).
3. **Software-RT fallback tier** — Lumen degrades through *software* ray tracing before giving up
   (`ue5:78`); smallworld degrades straight to SSAO+probe. See §3.2 for why core needs its own
   answer here rather than borrowing the Voxel Plugin's SVO.
4. **Virtual Shadow Maps** — mentioned as "(future)" (`:174`), fine; but combined with §3.1
   (volumes cast no shadows at all), shadows are the weakest subsystem in the doc.
5. **Frame pacing / latency control** — the research doc covers max frame latency and
   Reflex-style pacing (`ue5:28-29`); the design doc's latency story ends at Lockstep mode. No
   frame pacing, no GPU-bound throttling policy.
6. **Translucency lighting & volumetrics** — UE5 treats volumetric fog and lit translucency as
   first-class (`ue5:82-87`). Smallworld's transparency section is three sentences, transparent
   objects' lighting model is unspecified (forward with clustered lights? unlit?), and — notably
   for a *voxel* engine — there is no participating-media story at all: no fog, no smoke-like
   translucent volumes. `VolumePass` writes opaque GBuffer pixels; a semi-transparent volume fits
   neither the opaque nor the mesh-transparency path.

---

## 7. Gaps and omissions (beyond UE5 comparison)

The "UE5 in Rust" positioning raises the bar here: for UE5, the subsystem stack *is* the product.
Not every item below needs a design now, but the architecture doc should state a stance for each —
and the structural choices above (retained scene, multi-view, GPU-driven contracts, pass
participation) determine whether they can be added later without rewrites.

**Rendering:**

- **Seamless LOD transitions** — the doc says nothing about transition mechanics
  (cross-fade/dither for meshes in core; brick-resolution blending and the
  extracted-mesh↔raymarch handoff for the Voxel Plugin, whose headline consumer — battlemoon —
  names seamless LOD as critical). LOD *selection* is mentioned; LOD *transition* is the hard
  part, and the `GeometryBackend` contract says nothing about how a backend implements one.
- **Volume motion vectors** — TAA needs velocity for volumes; raymarched pixels have no vertex
  motion. Unaddressed → ghosting on every moving volume.
- **IBL / reflection probes / sky lighting** — PBR without an image-based specular term looks
  flat; `EnvironmentParams` is never defined, "ambient probe" appears only in the fallback table.
- **Auto-exposure (eye adaptation)** and **upscaling** (TSR/FSR-class; the post-process research
  notes compare these) are absent from the post chain. Upscaling is the single biggest perf lever
  for an expensive-per-pixel raymarching engine.
- **Decals** — no story.
- **Skinned meshes** — mentioned once (per-frame BLAS rebuild, `:194`) but there is no skinning
  design: where does skinning run (compute pre-skin vs. vertex shader), how do skinned verts feed
  the depth prepass, motion vectors, and BLAS refit consistently?
- **Resize / surface-lost / device-lost** — swapchain recreation crosses the thread boundary;
  `Engine::run() -> !` plus `App::shutdown` implies teardown ordering (render thread drain, GPU
  idle) that is never specified.

**Engine systems:**

- **Physics** — a `RigidBody` component and "physics broadphase" on workers, but no architecture:
  which engine (rapier?), how it syncs with `Transform`, fixed-step ownership, worker-pool
  interaction with the render thread's culling jobs (shared rayon pool → priority inversion:
  render-critical culling can queue behind physics; UE5's task system has priorities, the doc's
  worker pool doesn't).
- **Streaming** — Principle 5 promises explicit streaming budgets; `BrickResidencyInfo` and
  `StreamPriority` are name-dropped; there is no streaming section. A World Partition analog is
  core-engine territory, and the OOC voxel pipeline is its first heavy customer — streaming
  arguably deserves more design than ray tracing got.
- **UI** — nothing. The research doc covers Slate/UMG (`ue5:333-338`); a "complete game engine"
  needs at least a stance (immediate-mode overlay? egui integration? custom retained UI?).
- **Networking** — nothing, not even "out of scope v1." Given battlemoon, a sentence is owed.
- **Save/serialization** — descriptors spawn entities; nothing about serializing a live World.
- **ChangeTracker vs. resources** — materials are mutable via `ResourceHandle` but the tracker
  only covers entities/components; what marks a material dirty for `UpdateMaterial`? (`:1261-1275`)

---

## 8. Suggested discussion agenda (ranked)

*(Superseded by the Disposition section — items 1–4 and 6–7 are resolved, item 5 partially;
kept for the record.)*

1. **Retained render scene vs. snapshot packets** (§1.2) — this decision cascades into extract,
   change tracking, caching, and packet format. Everything else waits on it.
2. **Split `GeometryBackend` across the thread boundary** (§1.1) — API-breaking later, cheap now.
3. **Pass-participation contracts** (§1.4, §1.3, §3.1) — the common-currency question: a mesh
   interop stream plus participation traits, so plugin geometry can exist in shadows, depth, HZB,
   TLAS, and velocity. Make-or-break for the "shipped Voxel Plugin" strategy; the §2.1
   depth-write mechanism is its prerequisite.
4. **Multi-view / shadow-view culling** (§3.4) — packet format change; cheap now, painful later;
   prerequisite for item 3's shadow participation.
5. **RT scope honesty + core GI fallback stance** (§2.2, §2.3, §3.2) — rewrite the RT section
   against what `ray_query` actually provides; design the surface cache or the bindless fetch
   path; decide core's software tier (accept the SSAO+probe cliff vs. an SDF/SSGI tier) and make
   the lighting pass's GI/shadow-mask inputs a public contract plugins can feed.
6. **Instancing + GPU-driven contracts** (§3.3) — at minimum make `MeshDrawCommand` instancing-
   capable and pools dense instead of HashMap.
7. **Readback ring for feedback** (§2.4) — small design, removes a false invariant.
8. **Gameplay-layer mechanics** (§5) — script command buffers, event double-buffering, fixed-step
   interpolation, `AssetServer` mutability.
9. **Doc hygiene** — reconcile with CLAUDE.md's entity-model text and the implemented GBuffer
   contract; deduplicate `AccelerationStructure`; fix the small inconsistencies in §4.
