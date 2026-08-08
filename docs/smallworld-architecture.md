# smallworld — architecture after selection

Where the technique selections land in the `Resolve → Cull → Stream → Execute`
pipeline, and what they demand of the infrastructure underneath it.

## Three structural consequences

1. **The four stages run over two asset kinds, not one.** Bricks and meshes both get
   resolved, culled, streamed and executed. Every stage gains a mesh-side substage,
   and the shared structures (world skeleton, AABB buffer, budget controller) need a
   kind tag rather than a second parallel system.
2. **Execute is no longer one stage.** With two GBuffer producers, a composite, a
   two-term shadow, a GI gather, clustered lighting, volumetrics and a post chain, it
   holds most of the frame. It decomposes into geometry / lighting / post.
3. **There are two feedback loops, not one.** DESIGN.md has depth → HZB. TAA, SSR,
   SSGI and GI probes add a second: a set of previous-frame resources with identical
   lifetime and invalidation rules. Treat them as one owned concept, not five
   ad-hoc history textures.
4. **The four stages are the render pipeline, not the frame.** They're governed by
   data residency — disk → RAM → VRAM → pixels. Input and simulation are governed by
   tick rate and determinism. Making them substages of Resolve would bind the water
   CA and field diffusion to the render frame rate. They belong in an envelope around
   the four stages.

## The frame

Frames in flight: **3**. Sim(N+1) | Render(N) | GPU(N−1), matching UE5. Simulate and
command building run concurrently, so the renderer never touches `World` — it reads a
`FrameView` snapshot taken at the extraction point between them.

```
  ┌─ INPUT ──────────┐  event pump, UI capture, snapshot swap    ─┐
  │                  │                                            │
  ├─ SIMULATE ───────┤  fixed timestep, 0..n ticks per frame      │  Sim
  │   scripts        │                                            │  (N+1)
  │   entities       │                                            │
  │   physics        │                                            │
  │   fields, water  │  ──▸ EDIT QUEUE ──┐                        │
  │   weather        │                   │                       ─┘
  └──────────────────┘                   │
  ═══════════════ EXTRACT ▸ FrameView ═══╪══════════════════════════
             ┌──── temporal resources (N-1) ────────────────────┐
             ▼                           ▼                      │
  ┌─────────┐    ┌──────┐    ┌────────┐    ┌──────────────────┐  │  ─┐
  │ RESOLVE │───▸│ CULL │───▸│ STREAM │───▸│ EXECUTE          │──┘   │ Render
  │         │    │      │    │        │    │ geometry ▸       │      │ (N)
  │         │    │      │    │        │    │ lighting ▸ post  │      │
  └─────────┘    └──────┘    └────────┘    └──────────────────┘      │
                     ▲                              │                │
                     └───────── depth → HZB ────────┘                │
                                                    ▼                │
                                        ┌─ UI DRAW ─┬─ PRESENT ─┐    │
                                        └───────────┴───────────┘   ─┘

                     ── meanwhile: GPU executes frame N-1 ──
```

Only Cull and Execute are hard per-frame work. Resolve and Stream are budgeted and
allowed to lag — that asymmetry is what the job priority tiers exist to express.
Simulate runs a fixed-step accumulator and may tick zero or several times per frame.

**The edit queue is the interface between simulation and rendering.** Voxel writes
from scripts, physics, destruction and water flow must be collected into one applied-
at-a-sync-point command list rather than scattered across subsystems, because
everything downstream keys off it: dirty bricks trigger re-meshing (1.2), re-upload
(3.2), BLAS invalidation (3.5), GI cascade refresh (4b.2), and field/water active-set
expansion. It is double-buffered and swapped at the extraction point, since Simulate
is filling N+1's queue while Render still consumes N's.

---

## Phase A — Engine boot

Runs once per process. World-independent — nothing here knows whether a world will
ever be mounted.

| Step                          | Work                                                                                                | Why the position                                 |
| ----------------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| A.1 Instance, adapter, device | Request features; build a `Capabilities` struct                                                     | **Everything gated flows from here** — see below |
| A.2 Job pools                 | I/O and compute pools, named threads                                                                | Everything below is multi-threaded               |
| A.3 GPU pools                 | Brick pool, mesh pool, staging ring, optional AS pool; sized from device limits and the VRAM budget | Must precede any upload                          |
| A.4 Pipeline compilation      | Shader variants per capability; warm the cache                                                      | Async — the classic first-frame stall            |
| A.5 Subsystems                | Audio device, input devices, script runtime + binding registration, UI theme                        | Independent; can overlap A.4                     |

**Capability negotiation is the architectural payoff of the wgpu 30 situation.** Ray
query, mesh shaders, `SHADER_I16` and HDR color spaces are each present on some
backends and absent on others. Resolving them once at A.1 into a struct that selects
pipeline variants turns every gated decision in the selection document from a
compile-time commitment into a runtime branch — and it means the fallback path (SVO
software traversal, indirect draw, LDR output) is exercised on real hardware rather
than rotting.

## Phase B — World mount and unmount

Runs every time a world appears or disappears: title screen backdrop, new game, level
transition, return to menu.

**Mount**

| Step                | Work                                                                                                                  |
| ------------------- | --------------------------------------------------------------------------------------------------------------------- |
| B.1 Skeleton build  | Coarse SVO: multi-threaded height sampling, min/max mip pyramid, air / buried-solid / surface-crossing classification |
| B.2 Residency prime | Fill the always-resident coarsest LOD                                                                                 |
| B.3 Ready signal    | World is presentable once B.2 completes; everything else refines in                                                   |

The always-resident-coarsest-LOD policy pays off here for free: it gives you a precise
definition of "loaded enough to show" without a separate loading heuristic. Detail
streams in afterwards under the normal Stream budget.

**Unmount** — the direction that gets written wrong:

1. Cancel in-flight stream requests (cancellation tokens already exist).
2. **Wait for GPU completion** before recycling ring segments — drive the completion
   counter forward until it covers every in-flight submission, since the GPU may
   still be reading staging memory for a world you're discarding.
3. Invalidate every temporal resource (see below) and any acceleration structures.
4. Reset the budget controller's accounting — this is the panic eviction path with a
   different trigger.

Surviving a mount cycle: device, pools, pipelines, job threads, audio, input, UI theme.
Not surviving: skeleton, pool _contents_, temporal resources, AS, script world state.

### The empty world is a valid state

A title screen with no world is a frame where the AABB buffer has zero entries. Resolve
finds nothing, Cull culls nothing, Stream moves nothing, and Execute's two GBuffer
producers write nothing — the composite falls through to clear colour or skybox, and
4d draws the UI over it.

Write the stages to be no-op safe rather than branching on game state. The moment the
code says `if in_game { render_world() } else { render_menu() }`, you own two renderers
and they will diverge. With no-op-safe stages you get, at no extra cost:

- **Menu over a backdrop world** — an ordinary frame with input routed to UI and a
  scripted camera.
- **Pause screen** — the same thing, with Simulate not ticking.
- **Level transition** — unmount, render empty frames, mount.

What actually differs between these states is not rendering. It's three things: who
has input priority, whether Simulate ticks, and what VRAM budget the world gets — a
menu backdrop should be capped low so mounting the real world doesn't fight it for
residency.

**Use the menu to hide A.4.** Pipeline compilation and initial residency prime both
want to happen while something is already on screen. A title screen is not dead time;
it's the natural cover for the two slowest parts of startup.

## Stage 0 — Input and Simulate

### 0a Input

Raw event pump on the main thread (winit). Two ordering contracts:

1. **UI gets first refusal.** Panels capture events before gameplay sees them, or
   text fields eat WASD.
2. **Simulate reads a snapshot, not the live queue.** Double-buffer the input state so
   a fixed-step tick sees stable input regardless of how many events arrived.

### 0b Simulate — fixed timestep

| Substage              | Work                                                                                          | Notes                                           |
| --------------------- | --------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| 0b.1 Script tick      | Rhai/Lua, budgeted per frame                                                                  | Budget enforcement here, not in the render loop |
| 0b.2 Entity update    | Gameplay logic, spawn/despawn, transforms                                                     | SlotMap mutation                                |
| 0b.3 Physics          | Rigid body solve; entity-vs-world swept AABB against the grid; entity-vs-entity BVH + GJK/EPA | Fixed step is non-negotiable                    |
| 0b.4 Field simulation | Diffusion over the active brick set; convergence prunes inactive bricks                       | Tick rate can be a fraction of the sim rate     |
| 0b.5 Water            | Cellular automaton over active water bricks; flow modulated by wind                           | Emits voxel edits                               |
| 0b.6 Weather          | Wind vector, fog density, precipitation, time of day → uniform buffer                         | Consumed by 4b.4                                |
| 0b.7 Edit application | Drain the edit queue; mark dirty bricks, fields, active sets                                  | The sync point                                  |

Audio isn't a substage — it's a realtime named thread fed by a serial-pipe command
queue from 0b.2 and 0b.5. Mixing must never be gated on a simulation tick.

Fields and water at 0b.4–0b.5 are the reason the fixed step matters: both are
iterative solvers whose behaviour changes with step size, so binding them to a
variable render rate makes water flow speed a function of frame rate.

## Stage 1 — Resolve

Populate the world skeleton. Mostly off the critical path, with one exception.

| Substage            | Work                                                                        | Where it runs              | Deadline           |
| ------------------- | --------------------------------------------------------------------------- | -------------------------- | ------------------ |
| 1.1 Volume resolve  | `BrickSource`: GPU worldgen → disk region cache → CPU fallback              | Compute pool + GPU         | Background         |
| 1.2 Mesh extraction | Marching Cubes / Dual Contouring from bricks; meshlet clustering; DAG build | Compute pool               | Background         |
| 1.3 Entity pose     | Skeletal sampling, blend tree evaluation, IK solve → bone matrices          | Compute pool, parallel-for | **Frame-critical** |
| 1.4 Registration    | AABBs into the unified buffer; skeleton state transitions                   | Main + workers             | Frame-critical     |

1.3 is the odd one out: it must complete every frame while 1.1 and 1.2 may take
several. Same pool, different priority tier — this is precisely the starvation case
that priority promotion covers, and why the FIFO pool option was dropped.

## Stage 2 — Cull

Entirely GPU compute, one command buffer, reading the unified AABB buffer.

| Substage                  | Work                                                                                     | Reads                     | Writes                      |
| ------------------------- | ---------------------------------------------------------------------------------------- | ------------------------- | --------------------------- |
| 2.1 HZB build             | Mip chain, max-depth downsample                                                          | Depth(N-1)                | HZB pyramid                 |
| 2.2 Frustum cull          | 6-plane AABB test over all kinds                                                         | AABB buffer               | Visibility bitfield         |
| 2.3 Occlusion cull        | Project AABB, sample coarsest covering HZB mip. Conservative                             | HZB                       | Visibility bitfield         |
| 2.4 SSE evaluation        | `voxel_scale × focal / distance`; sub-pixel threshold is contribution culling, free here | Skeleton LOD meta         | LOD demand, stream priority |
| 2.5 Meshlet cull          | Bounding sphere + normal cone; parent/child DAG selection                                | Meshlet descriptors       | **Indirect draw buffer**    |
| 2.6 Light cluster binning | Light volumes → froxel cells, log-depth slices                                           | Light list                | Per-cluster light indices   |
| 2.7 Shadow caster cull    | Entity set against the sun ortho frustum                                                 | AABB buffer (entity kind) | Shadow draw list            |
| 2.8 Visibility readback   | Diff visible vs resident                                                                 | Visibility bitfield       | Stream + eviction lists     |

Two placements worth noting. **Light clustering belongs here**, not in Execute — it's
a cull-class operation over bounding volumes and shares the AABB machinery. And **2.7
exists because the shadow map is entity-only**; it needs its own frustum and its own
visibility set, separate from the camera's.

2.8's readback must **never block**. Dispatch during frame N, consume during frame
N+2 — at depth 2 the GPU is only finishing N while the CPU builds N+1, so the result
isn't back any sooner. A `map_async` plus wait here collapses the pipeline to depth 1
and quietly undoes the whole arrangement. Everything above 2.8 is same-frame.

## Stage 3 — Stream

| Substage                              | Work                                                                                                                                                                                                                                                      |
| ------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 3.1 Request ordering                  | Sort by (visible × SSE); cancellation tokens for stale loads                                                                                                                                                                                              |
| 3.2 Ring buffer staging               | Workers lock a segment, write, release; wraparound                                                                                                                                                                                                        |
| 3.3 Copy submit                       | Submission is async; recycling is gated by a completion counter, not a fence — wgpu exposes none. Track a submission index per frame, advance "GPU completed through K" from `Queue::on_submitted_work_done`, recycle a segment when `K >= segment.frame` |
| 3.4 Budget + eviction                 | Hard VRAM/RAM caps; LRU weighted by SSE and visibility age; panic path on teleport                                                                                                                                                                        |
| 3.5 Acceleration structures _(gated)_ | BLAS build on brick edit / region load; TLAS refit per frame                                                                                                                                                                                              |

3.5 is the prototype gate. If AABB-BLAS build cost fits inside the budget
controller's per-frame allowance, it slots in here as another budgeted upload class —
same ring buffer, same eviction accounting. If it doesn't, the substage doesn't exist
and software SVO traversal carries everything downstream.

The budget controller now arbitrates three pools — bricks, mesh data, and possibly
acceleration structures — under one VRAM ceiling. That's a single-owner decision, not
three independent limits.

## Stage 4 — Execute

### 4a — Geometry

| Pass                     | Notes                                                                                |
| ------------------------ | ------------------------------------------------------------------------------------ |
| 4a.1 Entity shadow depth | Ortho, entity meshes only. Never meshed near-field terrain — it's already in the SVO |
| 4a.2 Rasterizer GBuffer  | `draw_indexed_indirect` (or mesh shader dispatch) from 2.5                           |
| 4a.3 Raymarcher GBuffer  | Compute; per-`VolumeKind` traversal kernels                                          |
| 4a.4 Composite           | Depth-aware merge; writes the source flag                                            |

4a.2 and 4a.3 are independent and can overlap. 4a.4 is the barrier.

### 4b — Lighting

Everything here reads the composited GBuffer and is representation-agnostic. That is
the payoff of the deferred choice — the two paths stop existing after 4a.4.

| Pass                    | Notes                                                                                                  |
| ----------------------- | ------------------------------------------------------------------------------------------------------ |
| 4b.1 Shadow term        | `min(svo_raymarch, entity_map)`. Ray origin offset ~1 voxel along normal when source flag = rasterizer |
| 4b.2 GI gather          | Cone trace against SVO mips / probe sample. Cone-traced AO falls out here, replacing SSAO              |
| 4b.3 Clustered lighting | Froxel light lists from 2.6                                                                            |
| 4b.4 Volumetrics        | Height fog, god rays, clouds. Reuses 4b.1's shadow term along the march                                |
| 4b.5 Water              | Refraction + Beer-Lambert transmission segment                                                         |

### 4c — Post

`SSR/SSGI → TAA resolve → bloom → motion blur → DoF → FSR upscale → tone map + grade`

TAA sits early because everything after it wants a resolved, stable image; SSR and
SSGI sit before it because they're the noisiest consumers of history. Upscale before
tone mapping so grading operates at output resolution against the HDR surface.

### 4d — UI and present

| Pass         | Notes                                                                                                                        |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| 4d.1 UI draw | Game UI and the egui debug overlay. After tone mapping — UI is authored in display space and shouldn't be graded or upscaled |
| 4d.2 Present | Surface acquire, submit, present. Frame pacing lives here                                                                    |

UI has two touchpoints in the frame, not one: input routing at 0a and drawing at 4d.1.
Layout can happen in either — at 0a if widgets react to input this frame, at 4d.1 if a
frame of latency is acceptable.

`Surface::get_current_texture` returns an enum in wgpu 29+ covering timeout, occluded,
outdated, suboptimal and lost. Handling those states is where the swapchain
reconfigure and device-lost recovery paths live, and it's easy to leave as an
`unwrap()` that ships.

---

## Cross-cutting structures

### The GBuffer contract

The central interface of the whole engine. Both producers write it; every lighting and
post pass reads it. Fix it early.

| Channel               | Written by | Consumed by                                   |
| --------------------- | ---------- | --------------------------------------------- |
| Depth                 | Both       | HZB(N+1), shadow, SSR, volumetrics, DoF       |
| Albedo                | Both       | Lighting                                      |
| Normal                | Both       | Lighting, shadow bias, SSR, GI                |
| Roughness / metallic  | Both       | Lighting, SSR                                 |
| Emissive              | Both       | Bloom, GI injection                           |
| **Velocity**          | Both       | TAA, motion blur                              |
| **Source flag**       | Composite  | Shadow ray offset, any path-specific handling |
| Material / palette id | Both       | Debug, material overrides, water detection    |

The last two are the ones that don't exist yet and are painful to add later.

### Frames in flight

`FRAMES_IN_FLIGHT = 3`. Three rules:

1. **Every GPU resource the CPU writes per frame is an array indexed by
   `frame_index % FRAMES_IN_FLIGHT`.** Cluster light lists, indirect draw buffers,
   instance transforms, uniforms, staging segments. Otherwise a later frame's writes
   land in memory the GPU is still reading.
2. **No CPU wait on GPU results inside a frame.** Any blocking map collapses the
   pipeline to lockstep. The tempting place is 2.8.
3. **Resolve → Execute never reads `World`.** It reads a `FrameView` produced at the
   extraction point between Simulate and Resolve. Simulation owns `World`; the
   renderer owns its snapshot.

Sized by this constant: the staging ring (matching DESIGN.md's original "N frames in
flight, typically 3") and every per-frame buffer above.

**What extraction copies.** Camera, light set, instance transforms, bone matrices
from 1.3, the weather uniform, the drained edit queue, and the `ChangeSet` deltas the
renderer needs to update its GPU-side scene. With no gameplay simulation yet this is
small and effectively free — which is the point of building it now rather than
retrofitting it once physics, scripts, fields and water are all writing into `World`.

**The trade being accepted.** Until the simulation band has real work, the third band
is empty: depth 3 costs roughly one extra frame of latency over depth 2 and buys no
throughput yet. The payoff arrives with physics and scripting, and the alternative —
migrating later — means touching every render-side call site.

Keep depth 1 as a runtime toggle. A GPU fault at depth 3 references data the CPU
overwrote two frames ago; being able to drop to lockstep is worth the flag.

### Temporal resources

One owned set, double-buffered, invalidated together:

- Depth (→ HZB)
- Color history (TAA)
- GI probe irradiance
- SSR / SSGI history

They share a trigger: camera teleport invalidates all of them, and that's the same
event as the budget controller's panic eviction path. Wire them to one signal rather
than discovering the coupling later as a set of one-frame artifacts.

### Making volumes and meshes first-class

The move is a **kind tag on a shared entry**, not two parallel systems:

```
AabbEntry {
    aabb: Aabb,
    kind: RenderableKind,   // Volume(VolumeKind) | Meshlet | EntityInstance
    payload: u32,           // index into BrickPool / MeshPool / instance array
    lod: LodMeta,
    flags: u32,             // resident, shadow caster, dirty
}
```

Consequences that fall out for free:

- Cull (2.2–2.4) is one dispatch over one buffer; kind only matters at output, where
  visibility fans into per-kind lists.
- The world skeleton's residency states (`Unknown → Loading → Resident → MipOnly`)
  apply unchanged to meshlets — a meshlet DAG level is a LOD tier like an SVO mip.
- One budget controller sees all GPU memory because everything suballocates from
  pooled buffers: `BrickPool`, `MeshPool` (vertex/index arena + meshlet descriptors),
  and optionally the AS pool.
- `BvhAccel<T: Bounded>` already spans all three uses — ChunkedVolume macro structure,
  entity instances, meshlet clusters.

The `Renderer` trait stays as designed. The `VoxelVolume` trait gains a mesh-side
counterpart only if mesh assets need traversal-time polymorphism — they probably
don't, since a meshlet is a meshlet regardless of whether it came from Dual Contouring
or an authored asset.

### Job system mapping

| Stage / substage       | Pool                  | Tier                                           |
| ---------------------- | --------------------- | ---------------------------------------------- |
| 1.1 worldgen, disk     | I/O (2–3) + compute   | Background                                     |
| 1.2 meshing, DAG build | Compute (4–6)         | Background, promoted when the chunk is visible |
| 1.3 pose evaluation    | Compute, parallel-for | Frame-critical                                 |
| 3.2 staging writes     | I/O                   | Frame-critical                                 |
| Audio mix              | **Named thread**      | Realtime — never on a stealing pool            |
| Render submission      | **Named thread**      | Realtime                                       |

Dependency graph edges follow the pipeline: worldgen(X) → meshing(X) → upload(X), with
atomic prerequisite counters resolving them. Serial pipes carry the ordered
per-subsystem queues (audio commands, stream requests) without dedicating threads.
