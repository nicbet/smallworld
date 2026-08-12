## The Render Thread

The Render Thread receives a `FramePacket` each frame and translates it into GPU work through a structured sequence of passes organized by a render graph.

### 1. Receive & Prepare

The Render Thread blocks on the channel until a `FramePacket` arrives. It then processes any `ResourceOp` entries — uploading new meshes and textures to GPU pools, updating material uniform data, and freeing resources for despawned entities — and applies the packet's scene deltas to the retained `RenderScene`: upserting and removing commands in the shared mesh draw store, and handing each backend's custom payload to its renderer half.

This is the only point where the Render Thread mutates its persistent state (GPU resource caches and the `RenderScene`). Everything after this is read-only traversal of the scene and packet.

### 2. Visibility & Culling

Before anything is drawn, the engine determines what each view can see. Culling is CPU-driven in v1 (worker-pool parallel); GPU-driven culling with indirect draws is the sanctioned phase-2 path (OQ 7) and must slot in additively. Culling is **per-view**: the packet's main and auxiliary views, plus shadow views the Render Thread derives from the shadow-casting lights (cascade fitting against the main view — the equivalent of UE5's InitViews shadow setup). The **engine itself culls the shared mesh store** — it owns the store; flat AABB tests, worker-pool parallel. Each registered geometry renderer culls only its _custom-lane_ retained data per view via `renderer.cull()`, allowing specialized strategies (e.g., octree traversal for volumes). `MeshBackend` therefore needs no renderer half at all — its geometry lives entirely in the engine-culled shared store.

- **View setup.** Collect this frame's views: main camera, aux views (render-to-texture, probes), and one view per shadow cascade / shadowed local light.
- **RT instance collection.** When RT is enabled, the TLAS instance list is gathered **before any per-view culling**, from the retained scene under its own, larger culling domain (an RT radius around the camera — off-screen geometry must still exist for shadows, reflections, and GI). See the Ray Tracing section.
- **Frustum culling.** Test every draw command's world-space AABB against each view's frustum planes. Parallelized across the worker pool.
- **Occlusion culling (HZB).** Using the Hierarchical Z-Buffer built from the previous frame's _final opaque depth_ — all backends, not just meshes (see the GBuffer stage) — test remaining objects in the main view to discard those hidden behind large occluders.
- **LOD selection.** For volumes and meshes with LOD levels, select the appropriate detail tier based on screen-space size or distance. Each backend owns its LOD strategy. Selection uses **hysteresis** — separate up/down thresholds — so boundary-distance oscillation cannot ping-pong transitions; transitions themselves use the fade/dither contract (see the Mesh Drawing Pipeline) and **gate on residency** (see Streaming).
- **Sort & batch.** Per view: opaque draws sorted front-to-back (minimize overdraw), transparent draws back-to-front. Draws sharing pipeline state are merged into instanced batches.

### 3. Deformation (`DeformPass`)

_(OQ 14 resolution, 2026-08-11.)_ All GPU vertex deformation — skinning first among it — runs once per frame in a compute stage before any geometry pass. Downstream, deformed geometry is indistinguishable from static geometry.

- **Compute pre-skin (skin cache).** For each skinned instance in this frame's deformation domain — the union of the per-view cull results, **plus the RT-eligible set (inside the RT culling radius) when RT is active**, so BLAS refit never consumes stale vertices — a compute dispatch applies the bone palette and writes deformed positions/normals/tangents into a per-instance output vertex buffer. Depth pre-pass, GBuffer, every shadow view, and BLAS refit all consume that buffer — skin once, consume everywhere; **one skinning implementation serves raster and RT.**
- **Deformers are an extension point.** Skinning is the built-in deformer; morph targets, cloth, and procedural deformation register as additional compute deformers writing the same output buffers (the UE5 Deformer Graph shape). Plugin-friendly by construction.
- **Velocity via buffer aliasing — no shader permutations.** Geometry vertex shaders always read a `position_prev` attribute and multiply by `prev_world_matrix`. Rigid draws bind the _same_ position buffer as `position_prev` (all motion comes from the matrix); deformed draws bind last frame's deformed output (pose motion), with the matrix carrying object motion. One shader, both cases, exact skinned motion vectors. Deformed outputs are double-buffered for this.
- **Budgeted (Principle 5).** Deformed-output memory (~40 B/vertex × 2 buffers × instance) is a named budget; over budget → LOD down or cap deformed instances. There is deliberately **no vertex-shader fallback path** — one implementation, per the permutation-as-optimization-never-architecture rule; a fallback is added only if a shipped need demonstrates it.
- **Animation sampling stays on the CPU** (worker pool): blend trees, IK, and clip evaluation are game-state logic (see the Animation section). Bone palettes (~6 KB per character) upload per frame via the staging pool. The GPU deforms; it never runs animation logic.

### 4. Depth Pre-Pass

Establishes the scene's depth early to prevent overdraw in the full GBuffer pass.

- **Early Z.** Opaque meshes render depth-only to the Z-buffer.

The HZB is deliberately _not_ built here — a pre-pass HZB would contain only mesh depth, and geometry from other backends (raymarched volumes, custom plugins) would never act as occluders. It is built after the GBuffer stage, once every backend has contributed depth.

### 5. GBuffer Pass (Geometry)

Visible opaque surfaces write their material properties to the Geometry Buffer. Smallworld uses deferred rendering, separating geometry from lighting.

| GBuffer Target | Format         | Data                                                             |
| -------------- | -------------- | ---------------------------------------------------------------- |
| Albedo         | Rgba8UnormSrgb | Base color (RGB); A = shading model ID (4 bits) + flags (4 bits) |
| Normal         | Rgba16Float    | World-space normals (octahedral encoded)                         |
| Material       | Rgba8Unorm     | Roughness, metallic, reflectance, AO                             |
| Emissive       | Rgba16Float    | Self-illumination (RGB) + intensity                              |
| Velocity       | Rg16Float      | Per-pixel motion vectors (TAA, motion blur)                      |
| Depth          | D32Float       | Z-buffer                                                         |

Both rendering paths write to this same GBuffer:

- **Rasterized meshes** write via traditional vertex/fragment shaders through the `GBufferPass`.
- **Raymarched volumes** write via fragment-shader raymarching over rasterized proxy geometry through the `VolumePass`, exporting real depth via `frag_depth` (see the Volume Rendering Mechanism below).

The lighting pass and everything downstream never knows which path produced a given pixel. (A mesh/volume source bit exists among the albedo-alpha flag bits for debug tooling — but no lighting or post pass may branch on it.)

**Shading Model ID.** The albedo alpha channel carries a per-pixel shading model ID (up to 16 models). The lighting pass switches on it to select the lighting response — `Standard` (Cook-Torrance PBR), `Unlit`, and registered custom models (toon, foliage, …). This is UE5's per-pixel shading-model mechanism: it is what lets custom materials change _how light responds_, not just which material inputs are written.

**HZB construction.** After all opaque backends have written depth — rasterized meshes and raymarched volumes alike — the final opaque depth is downsampled into the HZB mip chain used by the next frame's occlusion culling. Building the HZB here (rather than in the depth pre-pass) means volumes and custom geometry act as occluders: a voxel mountain culls the city behind it.

#### Volume Rendering Mechanism (`VolumePass`)

_(OQ 1 resolution, 2026-08-11.)_ Raymarched volume tiers render as **fragment-shader raymarching over rasterized proxy geometry** — the Teardown-proven pattern, chosen because it gets depth testing, sRGB conversion, MRT writes, and shadow-view reuse from hardware, and runs on all supported GPUs:

- **Proxy geometry.** One AABB per volume object (or streaming chunk), rasterized in the `VolumePass`. The fragment shader marches the object's brick/SVO data via hierarchical DDA; the first hit writes all GBuffer targets, velocity, and `frag_depth`. Bricks of one object are traversed inside a single invocation (first-hit termination), so only inter-_object_ overlap pays overdraw.
- **Depth interop.** `frag_depth` export writes the real D32Float depth buffer; hardware depth testing resolves volume-vs-volume and volume-vs-mesh ordering. Because `frag_depth` forces late-Z (WGSL has no conservative-depth hint), the shader early-outs against `depth_mesh_copy` — a snapshot of the mesh pre-pass depth taken before the `VolumePass` (a compute copy; Depth32Float cannot be reinterpreted as R32Float in wgpu).
- **Motion vectors.** The hit point's world position is transformed by the volume's previous-frame transform and previous view-projection — the same velocity math as meshes, written in the same shader. (Rigid-motion approximation; animated voxel _content_ reads as changed data, not motion.)
- **Camera inside a volume.** Rasterize proxy back faces; clamp the ray start to the near plane.
- **Shadow casting.** The same shader compiled depth-only implements `ShadowCaster::render_shadow_depth` for each shadow view.
- **Distant tiers** are extracted meshes on the shared mesh stream — no raymarching at all.

**Volume LOD transitions (Voxel Plugin design — built entirely on public contracts, OQ 10):**

- **Within the raymarched tier: distance-banded blending, clipmap-style.** LOD rings around the camera with fixed blend bands; inside a band the raymarcher samples both LOD levels and lerps density/material by the distance-derived factor. Stateless and continuous under camera motion — entirely private to the raymarch shader, no engine involvement. One time-based rider: a brick arriving _late_ (residency-driven, not distance-driven) fades in over ~100–200 ms from its coarser parent, which the pinned-coarse invariant guarantees is present.
- **The extracted↔raymarched handoff: convergence + complementary dither.** Convergence is a content rule — the extracted mesh for distance D is extracted _from the same coarse brick LOD_ the raymarcher samples at D, so the handoff blends two renderings of nearly the same surface. The residual is hidden by a dithered cross-fade band: extracted-tier draws use the shared-stream `fade` field; the `VolumePass` dithers complementary via the public dither convention. Zero voxel-specific engine hooks — the API-sufficiency test passes again.

**Capability-gated upgrade tier (deferred to the GPU-driven work, OQ 7).** A compute visibility-buffer variant — depth+payload packed via 64-bit atomic min/max (`Capabilities::int64_atomic_min_max`; on Metal this requires Apple M2-class "Nanite atomics") — is the sanctioned future optimization, not a rejected option. It is deferred because it needs its own coverage/binning and resolve machinery, the fragment path must exist for baseline hardware anyway, and its win is unproven for large-box proxies (Nanite's software raster exists to beat micro-triangle raster inefficiency, which volume proxies don't have). **Adoption trigger:** profiling shows the fragment path limiting on capable hardware, or variable-rate marching is needed.

#### Picking & Debug IDs

_(OQ 22 resolution, 2026-08-11.)_ There is deliberately **no per-frame ID target** in the GBuffer — per-pixel IDs would pay ~4 bytes/pixel of write+read bandwidth every frame for consumers that are better served elsewhere:

- **Gameplay picking = CPU raycast** against the BVH on the Game Thread. Zero GPU involvement, zero latency, and it can hit entities the camera culled.
- **Tools/editor picking = on-demand pick pass**, scissored to a few pixels around the cursor. Each geometry path has an ID-output shader variant writing a tagged `PickId` (u32: 2-bit source tag + 30-bit payload — the mesh path writes the instance-slot index into the shared `InstanceData` buffer, which resolves uniquely to draw + instance; the volume path writes the entity index). Results return through the readback ring (~2-frame latency, fine for tools). The CPU resolves PickId → entity/material and **validates the entity generation**, so a pick landing after a despawn misses cleanly instead of hitting a recycled slot.
- **Debug views** (entity/material heatmaps) run the same pass full-screen, only while active.

Material identity has no dedicated storage anywhere — it is derivable: PickId → draw → `material_gpu_id`.

#### Deferred Decals (core feature, scheduled)

_(OQ 13 resolution, 2026-08-11.)_ Decals are a **core engine feature**, implemented as standard deferred GBuffer decals: projected box volumes rendered after opaque geometry and before lighting, reading depth to reconstruct the surface and blending albedo/normal/material contributions into the existing GBuffer targets (respecting `DrawFlags::RECEIVE_DECALS`). Albedo blending is **write-masked to RGB** — the alpha channel's shading-model/flag bits are never touched. Normal blending decodes, blends, and re-encodes the octahedral normal target. The pass is purely additive over existing contracts — no new render targets, no downstream changes — which is why implementation is safely scheduled after the v1 rendering core lands without any design debt accruing in the meantime.

### 6. Shadow Pass

The engine renders depth from the perspective of each shadow-casting light into a shadow atlas, using the per-shadow-view draw lists produced during culling — shadow views see geometry the main camera culled.

Two kinds of geometry feed each shadow view:

- **The shared mesh stream.** Every `MeshDrawCommand` flagged `CAST_SHADOW` renders into the atlas — regardless of which backend emitted it (the Voxel Plugin's extracted-mesh tiers included).
- **`ShadowCaster` participants.** Backends whose geometry has no triangle form implement the `ShadowCaster` participation trait to render depth into a given shadow view themselves (e.g., raymarched volume depth).

Light types:

- **Directional lights** use cascaded shadow maps (CSM) with configurable cascade count (1–4).
- **Point and spot lights** render into atlas sub-regions.
- **Virtual shadow maps** (future) would cache static shadow pages and only re-render where dynamic objects move.

### 7. Volumetrics (Froxel Media)

_(OQ 9 resolution, 2026-08-11.)_ Participating media — global fog, local fog volumes, plugin-injected media — is computed in a frustum-aligned voxel grid (**froxels**) and applied wherever depth is known. Pure compute, no capability gates. The classic four-stage pipeline (Assassin's Creed 4 / Frostbite / UE5 Volumetric Fog):

1. **Density injection.** Global exponential height fog (from `EnvironmentParams`) and `FogVolume` entities are splatted into the grid (density, albedo, emission, phase). Injection is a **public contract**: plugins and games register injector passes that add density/emission — the Voxel Plugin's far-tier smoke uses exactly this.
2. **Froxel lighting.** Per froxel: sample the clustered light grid + shadow atlas, evaluate in-scattering with a Henyey-Greenstein phase function.
3. **Temporal blend.** Reproject and blend with the previous frame's froxel volume (jittered sampling).
4. **Integration.** Front-to-back accumulation into a scattering/transmittance volume.

Consumers sample the integrated volume by pixel depth: the lighting pass applies it to all opaque pixels (meshes and raymarched volumes alike — both are in the GBuffer), the sky pass applies the far-field value, and translucent draws sample it in their forward shaders. Default grid ~160×90×64, quality-tiered, Rgba16Float. The froxel volume is **always bound** (cheap when empty), sidestepping the optional-binding question of OQ 2 for fog.

### 8. Lighting Pass (Deferred Shading)

A full-screen compute dispatch evaluates deferred shading by reading the GBuffer, shadow atlas, and light buffer. Each pixel's lighting response is selected by its GBuffer shading model ID — `Standard` is Cook-Torrance PBR; other registered models (toon, foliage, unlit) branch here.

- **Clustered light assignment.** Screen-space tiles × depth slices. Lights are assigned to clusters on the CPU. Each cluster stores up to 32 light indices.
- **Shadow evaluation.** Percentage-closer filtering (PCF) samples the shadow atlas per light.
- **Pluggable indirect inputs.** The lighting pass declares public input slots for an indirect-diffuse (GI) texture, per-light shadow masks, and **sky visibility**. Engine RT passes feed the first two when hardware RT is available — but all are a **public render-graph contract**: a plugin (e.g., the Voxel Plugin's SVO-traced GI and sky visibility) or a future software-GI tier feeds the same slots without touching the lighting pass. This is the GI upgrade path: each slot progressively supersedes the fallback below it.
- **Indirect diffuse chain** _(OQ 11, ladder completed by OQ 4)_: GI input slot when fed (RT GI → screen traces + GI clipmap → plugin GI) → sky SH9 irradiance × AO × sky visibility → constant ambient.
- **Indirect specular chain** _(OQ 11)_: RT reflections → SSR (**always-on**, not RT-gated) → GI-clipmap rough-specular cones (when the software GI tier is active — OQ 4) → local reflection probes (when present — deferred feature) → prefiltered sky cubemap × sky visibility.
- **Sky visibility is mandatory.** The environment term — specular _and_ diffuse — is always modulated by a sky-visibility factor, so interiors go dark instead of sky-mirrored. Floor (core): bent-normal/SSAO-derived specular occlusion — screen-space, no authoring, works for any game. Upgrades (public slot): cone-traced visibility from the core GI clipmap when the software tier is active (OQ 4), or the Voxel Plugin's SVO-traced directional visibility — exact and destruction-proof (carve the roof open; visibility updates the same frame).
- **Fog application.** Sample the integrated froxel volume at each pixel's depth and apply scattering/transmittance (see Volumetrics).
- **Output.** HDR lighting result written to an Rgba16Float texture.

#### Software GI Tier — the GI Clipmap

_(OQ 4 resolution, 2026-08-11.)_ When hardware RT GI is unavailable or disabled, core provides software GI: **screen traces first, then cone tracing against a lighting-domain voxel clipmap** — the SVOGI family, with shipped precedent in CryEngine's SVOGI carrying Kingdom Come: Deliverance 1 and 2 (open world, time-of-day, no bakes).

- **The clipmap.** Camera-centered cascaded 3D textures (opacity, albedo, normal, emissive) — a _lighting-domain_ voxelization, distinct from the Voxel Plugin's content SVO. Geometry enters by **conservative rasterization of the shared mesh stream** (any backend's triangles participate automatically — a pure-mesh game gets full GI with zero content changes) or through the **GI injection point**, a participation contract in the froxel-injection mold: the Voxel Plugin injects SVO data directly — more accurate than voxelizing extracted meshes, and destruction updates GI the same frame.
- **Geometry and lighting are separate steps.** Direct light injects into voxels each frame (sun via the shadow cascades, locals via the cluster grid), so time-of-day _relights_ without re-voxelizing; destruction re-voxelizes only touched clipmap regions.
- **Consumers.** Cone-traced indirect diffuse feeds the GI slot (half-res + temporal, same dispatch pattern as RT GI). **Cone-traced sky visibility** from the same structure upgrades the OQ 11 baseline for _all_ games (the bent-normal floor remains beneath it). **Rough-specular cones** slot into the reflection chain between SSR and the sky term — a middle rung it previously lacked.
- **Costs, on the record.** Thin-wall light leaking is the classic VCT artifact (finer near cascades and occlusion cones mitigate it; nothing eliminates it); clipmap memory is a named budget (~100–200 MB across cascades); quality sits below Lumen-class GI.
- **World-radiance role (OQ 3).** The clipmap is also the hit-radiance source for hardware RT (GI rays always; reflection rays when rough/distant), so it is maintained whenever _either_ the software tier or RT effects are active — one representation, both paths, exactly the role Lumen's surface cache plays. The v2/v3 surface cache inherits this role for both paths in the same swap.

The indirect-diffuse ladder in full: **hardware RT GI → screen traces + GI clipmap → sky SH × visibility floor** — every rung feeds the same public slots; changing rungs never touches a shader contract.

**Roadmap (v2/v3): the smallworld Lumen analog.** Mesh-distance-field + surface-cache GI — per-asset SDFs computed at import, an incrementally composited global SDF, radiance-cached surface parameterization — is the **committed quality end-state**, not a rejected option. The architecture is published and de-risked; the remaining cost is content-hardening (thin geometry, foliage, leak edge cases), not research. The upgrade is a **swap of the world representation behind the same public slots**: screen traces, temporal accumulation, and every consumer carry forward unchanged, and the clipmap likely survives as the far-field/fallback representation.

### 9. Ray Tracing (Secondary Effects)

Smallworld follows UE5's hybrid rendering model: rasterization handles primary visibility (what the camera sees), ray tracing handles secondary effects (how light bounces, reflects, and casts shadows). Rasterization writes the GBuffer; ray tracing reads it.

This section is conditional on `Capabilities::ray_query`. **The design targets wgpu's inline ray queries exclusively** — rays are traced from compute shaders via `ray_query`. wgpu exposes no ray-tracing pipelines, no shader binding table, and no hit/any-hit/intersection shaders, so nothing in this design may depend on them; on a hit, shaders receive instance/primitive indices and fetch surface data manually (see RT Global Illumination). When hardware RT is unavailable, the engine falls back to screen-space approximations (SSAO, SSR) or skips the effects entirely. The rest of the pipeline is unchanged — RT passes are optional render graph nodes.

#### Acceleration Structure

Ray tracing requires a spatial index on the GPU so rays can efficiently find intersections. This is a two-level hierarchy maintained by the Render Thread as part of `RenderState` (see Data Structures for the `AccelerationStructure` definition):

- **Bottom-Level Acceleration Structure (BLAS).** One per unique mesh geometry. Built from the vertex/index buffers already in `GpuMeshPool`. Rebuilt only when geometry changes — rare for static meshes; deformed geometry refits per frame against the `DeformPass` output buffers, the same skinned vertices the raster passes consume.
- **Top-Level Acceleration Structure (TLAS).** One per frame. References all BLAS instances with their world transforms. **Built before any per-view culling, from the retained scene** — never from a culled draw list. Rays need geometry the camera cannot see: off-screen shadow casters, the room behind the camera in a mirror. The TLAS therefore uses its own culling domain — an **RT culling radius** around the camera, larger than any view frustum (the same reason UE5 has a separate ray-tracing culling radius). `TlasContributor` backends add their instances here.

The TLAS build is a GPU operation — the Render Thread records it as a command before the RT passes execute. Cost is proportional to instance count, not triangle count, so it scales well.

Volume geometry has no triangle representation to feed into BLAS construction. Two strategies:

- **Extracted mesh BLAS.** The Voxel Plugin's extracted-mesh LOD tiers enter the TLAS like any other mesh draws — the shared mesh stream at work. Works today, coarser than the actual voxel data.
- **SVO compute raymarching (no RT hardware needed).** For voxel shadows and GI, the Voxel Plugin traces its own SVO directly in compute — no acceleration structure, no `ray_query` required — and feeds the results into the lighting pass's public shadow-mask / GI input slots. Hardware traversal of the SVO via custom intersection shaders is **not possible under wgpu** (no ray-tracing pipelines), so compute-side SVO tracing is the plugin's path. Direction agreed in principle; specifics tracked in Open Questions.

#### RT Passes

RT passes are standard `RenderPass` implementations that read the GBuffer and trace rays against the TLAS. They write to dedicated targets consumed by the lighting pass.

##### RT Shadows (`RTShadowPass`)

For each pixel in the GBuffer, cast one shadow ray toward each light source through the TLAS. Produces a per-light shadow mask — a binary (or soft-penumbra) occlusion value per pixel. Replaces or supplements the rasterized shadow atlas for lights that opt in.

- **Input:** GBuffer (depth, normal, position reconstructed from depth), TLAS, light buffer.
- **Output:** `rt_shadow_mask` — Rgba8Unorm; channels are assigned to the (up to 4) most significant RT-shadowed lights per cluster via the clustered light grid; lights beyond that fall back to the shadow atlas.
- **Dispatch:** Full-screen compute, 8×8 workgroups. One ray per pixel per light. Denoised temporally.

##### RT Global Illumination (`RTGIPass`)

Indirect lighting from light bounces. Cast rays outward from each GBuffer pixel based on a cosine-weighted hemisphere around the surface normal. There are no hit shaders under wgpu — `ray_query` returns instance and primitive indices on a hit — so hit-point radiance comes from **sampling the GI clipmap at the hit position** _(OQ 3 resolution, 2026-08-11)_: the lit clipmap is the engine's world-radiance representation, serving hardware RT and software GI alike (the Lumen surface-cache role at voxel fidelity). Coarse radiance is fine here — the cosine integration blurs it regardless. **Rule: the GI clipmap is maintained whenever either the software GI tier _or_ hardware RT effects are active.**

- **Input:** GBuffer, TLAS, GI clipmap (hit radiance).
- **Output:** `rt_gi` — Rgba16Float, indirect diffuse irradiance per pixel.
- **Dispatch:** Half-resolution (one ray per 2×2 quad), spatially and temporally denoised, then upsampled. Full-resolution GI is too expensive for real-time; the denoiser fills in.

##### RT Reflections (`RTReflectionPass`)

For pixels with low roughness, cast a reflection ray based on the GBuffer normal. Hit radiance is hybrid by ray character _(OQ 3)_: **rough or distant reflections sample the GI clipmap** (v1 — zero extra machinery); **sharp near reflections upgrade to bindless hit-shading** later — vertex/material fetch via binding arrays (capability-gated: `BUFFER_BINDING_ARRAY` / `TEXTURE_BINDING_ARRAY`), texture LOD via ray cones, with the noted caveat that off-screen hit points cannot use the view-space cluster grid and need a world-space light structure (or sun + IBL only) — deferred until sharp mirror quality demands it.

- **Input:** GBuffer (normal, roughness, depth), TLAS, GI clipmap (hit radiance).
- **Output:** `rt_reflections` — Rgba16Float, reflected radiance per pixel.
- **Dispatch:** Selective — only pixels below a roughness threshold. SSR runs regardless (it is always-on in the specular chain); RT results replace SSR where rays were traced. Rough surfaces and non-RT hardware resolve through SSR → probes → sky × visibility (see the Lighting Pass specular chain).

#### Compositing into Lighting

The `LightingPass` is extended to read the RT targets when they exist:

```
LightingPass reads:
    GBuffer (albedo, normal, material, depth)     — always
    shadow_atlas                                   — always (rasterized shadows)
    rt_shadow_mask                                 — when RTShadowPass ran
    rt_gi                                          — when RTGIPass ran
    rt_reflections                                 — when RTReflectionPass ran
    clustered_light_grid                           — always
```

When RT shadows are available for a light, they replace the shadow atlas sample for that light. RT GI adds to the ambient/indirect term. RT reflections replace or blend with the specular term based on roughness.

Optional RT inputs bind through the render graph's **optional-input-slot mechanism** (see Render Graph — Optional Input Slots): always-bound neutral dummies plus per-frame uniform flags — one lighting pipeline, uniform branching, no shader permutations.

This is pure additive integration — the rasterization pipeline produces a complete image on its own. RT passes improve quality when available but nothing breaks without them.

#### Render Targets (RT)

```rust
// Added to RenderTargets when Capabilities::ray_query is true
struct RTTargets {
    shadow_mask:    wgpu::Texture,  // Rgba8Unorm — per-light RT shadow
    gi:             wgpu::Texture,  // Rgba16Float — indirect diffuse
    reflections:    wgpu::Texture,  // Rgba16Float — specular reflections
    history_shadow: wgpu::Texture,  // Rgba8Unorm — temporal accumulation for denoiser
    history_gi:     wgpu::Texture,  // Rgba16Float — temporal accumulation for denoiser
    history_refl:   wgpu::Texture,  // Rgba16Float — temporal accumulation for denoiser
}
```

#### Fallback Path (No Hardware RT)

When `ray_query` is unavailable, the engine uses screen-space approximations in the same render graph slots:

| RT Pass            | Fallback                                                                    | Quality tradeoff                                   |
| ------------------ | --------------------------------------------------------------------------- | -------------------------------------------------- |
| `RTShadowPass`     | Shadow atlas only (rasterized CSM/atlas)                                    | No soft penumbra from RT, same shadows as baseline |
| `RTGIPass`         | Screen traces + GI clipmap (software tier — OQ 4); SSAO + sky floor beneath | Coarser bounce, VCT leak artifacts vs. RT          |
| `RTReflectionPass` | SSR (screen-space reflections)                                              | Misses off-screen reflections                      |

If the RT passes aren't registered (because `ray_query` is false), the lighting pass's RT input slots go unfed and the fallback terms are used.

The fallback ladder no longer cliffs _(updated by OQ 4)_: below hardware RT sits the **core software GI tier** — screen traces + cone tracing against the lighting-domain GI clipmap (see the Lighting Pass) — which assumes no particular scene structure: geometry enters by rasterizing the shared mesh stream. The SSAO + sky-IBL × visibility floor remains beneath it for minimal hardware. Voxel-heavy games improve the tier further through the GI injection point (the Voxel Plugin injects SVO data directly), and all of it flows through the same public input slots.

### 10. Sky & Atmosphere

Rendered into the HDR target where depth equals the far plane. Atmosphere scattering, procedural sky, or skybox cubemap. Applies the froxel volume's far-field scattering for atmospheric consistency with fogged geometry.

#### Environment Capture & IBL

_(OQ 11 resolution, 2026-08-11.)_ The sky is also the engine's image-based light source. The environment pipeline maintains three artifacts:

- **Prefiltered specular cubemap.** The sky (procedural or HDRI, per `EnvironmentParams::sky`) is captured to a cubemap and GGX-prefiltered into a roughness → mip chain.
- **SH9 irradiance.** The same capture projected onto 9 spherical-harmonic coefficients — the diffuse ambient term.
- **Split-sum BRDF LUT.** Generated once at startup.

Captures update amortized (one face or one prefilter mip per frame), so time-of-day costs a bounded slice of the frame budget. Consumers: the lighting pass (indirect chains), forward transparents (same prefiltered set), and the froxel lighting's ambient term. Local `ReflectionProbe`s (deferred — see Core Engine Components) reuse this exact capture/prefilter machinery at probe positions via aux views.

### 11. Transparency

_(OQ 9 resolution, 2026-08-11.)_ Objects with alpha blending render in a forward pass over the completed opaque image (lighting + sky). They cannot write to the GBuffer.

- **Clustered Forward+ lighting.** Transparent fragments shade with the same Cook-Torrance BRDF, the same clustered light grid, the same shadow atlas, and the same registered shading models as the deferred path — one light structure, two consumers, so lighting matches across the opaque/transparent boundary. The specular environment term is the same prefiltered sky set × sky visibility (see Environment Capture & IBL).
- **Refraction.** Glass and water sample `scene_color_copy` — an HDR snapshot taken after lighting + sky. A pass cannot sample the target it blends into.
- **Fog.** Each transparent fragment samples the integrated froxel volume at its depth.
- **Sorting.** Back-to-front per draw. No OIT in v1 — interpenetration artifacts are accepted (the shipped-game norm); weighted-blended OIT is a future quality knob.
- **Translucent media ≠ surface transparency.** Smoke/fire-like media renders in this stage but through different machinery: near-field hero media in plugin-owned raymarch passes (front-to-back march lit by the cluster grid + shadow atlas, composited by transmittance against scene depth — reads `depth`, never writes it), far-field media injected into the froxel grid. The opaque `VolumePass` is never used for media.

### 12. Post-Processing

_(OQ 12 resolution, 2026-08-11.)_ **Internal render resolution and display resolution are separate, first-class concepts.** All scene targets (GBuffer, HDR, froxels) allocate at internal resolution; the temporal resolve upscales; everything after it runs at display resolution. Dynamic resolution scaling is reserved in the contract: targets allocate at _maximum_ internal resolution and render at a per-frame scale carried in `ViewParams` — the control loop that drives the scale is frame pacing's job (OQ 8).

Pass order: auto-exposure histogram → temporal resolve/upscale → bloom → tone mapping → color grading → dev UI. (The histogram reads the _pre-upscale_ internal-res HDR buffer; its exposure output feeds both the temporal resolve and tone mapping.)

- **Temporal resolve & upscale (TAAU).** TAA and upscaling are one pass: jittered internal-res samples accumulate into a display-res history via motion-vector reprojection. Native-res TAA is TAAU at scale 1.0 — one code path, not two. The resolve is a **replaceable render-graph node** with declared inputs (HDR color, depth, velocity, exposure, jitter sequence) — exactly the interface vendor upscalers expect. Roadmap: **FSR 2.2 (WGSL port) in v2** through this slot; **DLSS when wgpu support is practical** (third-party integration crates like `dlss_wgpu` exist today; NVIDIA + Vulkan only).
- **Auto-exposure.** Histogram-based: a compute reduction over the _pre-upscale, internal-res_ HDR buffer (same statistics, fewer pixels); average within percentile clamps — outlier-proof metering, a sun pixel or black corner can't hijack it; **asymmetric adaptation speeds** (dark-adaptation slower, matching eyes); EV compensation and metering mask as artist controls. `Exposure::Manual { ev }` is an explicit mode for cinematic control; the mode lives per-camera (`Camera::exposure`). The current exposure value rides `FrameFeedback` as an advisory (night-vision-style gameplay uses).
- **Bloom.** Downsample bright regions (thresholds in exposed space), blur, composite back.
- **Tone mapping.** HDR → SDR/display HDR via **ACES** (default filmic transform).
- **Color grading.** Final LUT application.
- **Dev/debug UI.** egui renders as a final render-graph pass over the post-processed image (dev tooling — OQ 18).

### 13. Present

The final image is blitted to the swapchain surface. The Render Thread loops back to receive the next `FramePacket`.
