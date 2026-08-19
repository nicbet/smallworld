# Ch01-04 Engine-Comparison Audit (Phase 1)

**Date:** 2026-08-19
**Issue:** sw-438537
**Policy:** tiered comparison rule in `.claude/skills/book/SKILL.md` — full options-comparison + per-alternative winner rationale only where engines genuinely diverge AND Smallworld picks or invents (Tier 1). Ceiling example: ch05 Tick-Stamped Edges.
**Method:** ch03/ch04 read in full by the lead pass; ch01/ch02 swept by subagents against the Tier-1 rubric, final classification by the lead pass. Engine-behavior notes below are from working knowledge; every claim marked `[verify]` must be checked against vendor docs before Phase 2 writes prose.

## Chapter 03 — Runtime Contracts

### CH03-F1: Threading model (dedicated threads + split worker pools vs. the industry's alternatives) — **the largest retrofit in the audit**

- **Where:** "Thread Ownership and the Work-Stealing Pools" (§The Execution Contexts, §Why Two Worker Pools?).
- **Decision:** four dedicated threads (Game, Render, Audio, Streaming) + two split work-stealing pools + IO pool, CSP channels between domains.
- **Engines:** UE: named dedicated threads (Game/Render/RHI) plus a TaskGraph for parallel work `[verify current UE5 task system]`. Unity: main-thread orchestration + C# Job System worker pool, optional Graphics Jobs modes `[verify]`. Naughty Dog: fiber-based job system replacing dedicated per-subsystem threads (already cited, gyrling-gdc2015). Frostbite/id: task-graph schedulers (mentioned in passing, uncited).
- **Current prose:** rich historical grounding (Xbox 360 pressure, ND fibers) and a strong rationale for the *pool split* (priority inversion). But the strategies are presented as history, not as adoptable alternatives, and there is no per-alternative winner rationale for dedicated-threads+pools over a unified fiber/job/task-graph model.
- **Bonus inconsistency found:** the "What You Will Learn" bullet promises "why a unified task-graph scheduler is the planned evolution rather than the starting architecture," but no body text delivers that argument. The retrofit must add it (or the bullet must change).
- **Verdict:** compared-but-no-rationale. **Effort:** moderate — the one retrofit resembling ch05's ceiling. Comparison paragraph (fibers vs. task graph vs. dedicated threads+pools) + winner rationale + the promised evolution argument. ~300-400 added words; trim candidate: some duplication between the history paragraph and the execution-context walkthrough.

### CH03-F2: Resource lifetime — generational handles vs. GC vs. refcounting

- **Where:** "Handle-Based Resources".
- **Decision:** generational-index handles, no GC, no hot-path refcounting.
- **Engines:** UE: mark-and-sweep UObject GC + `TWeakObjectPtr` `[verify]`. Unity: managed C# GC for scripts + handle-like native asset instanceIDs `[verify]`. Godot: RIDs + `RefCounted` reference counting `[verify]`.
- **Current prose:** argues against refcounting and gives strong cross-domain analogies (file descriptors, Wayland IDs); GC as the alternative engine strategy is absent entirely.
- **Verdict:** compared-but-no-rationale (partial). **Effort:** small-moderate, ~150-250 words. **Recommended landing zone: Chapter 6** (memory/handles owns the deep treatment); ch03 gets one forward sentence. Execute with ch06's revision pass.

### CH03-F3: Extraction mechanism — proxies vs. server/RIDs vs. owned messages

- **Where:** "The Extraction Contract" / "General Properties and Cost".
- **Decision:** owned `FramePacket` messages over a bounded channel, delta extraction via dirty tracking.
- **Engines:** UE `FSceneProxy` + dirty push (cited); Godot `RenderingServer` + RIDs (cited); Unity absent — SRP/RenderGraph model, historically no user-visible game/render state contract `[verify]`.
- **Current prose:** already compares UE and Godot with citations, argues the four contract properties, and explicitly defers mechanism alternatives to Chapter 12.
- **Verdict:** already-compared-with-rationale, minor gap. **Effort:** small — one Unity line in ch03 (~80-120 words); full mechanism comparison stays owned by ch12 as the prose already promises.

### Ch03 rejected borderlines

- ECS vs. OOP game object model: Tier-1 fork, but **fully treated** (Bilas → Unity DOTS → UE Mass → Overwatch, citations, cost paragraph, explicit commitment). No action.
- CSP vs. shared-state locking: engines don't ship this as divergent user-facing options; the prose already argues the pick against locks. Adequate.
- Budget-explicit design: prose itself claims industry convergence ("every production engine... enforces similar limits"). Tier 2.
- Asset descriptors (text RON/JSON vs. binary .uasset): compared with rationale in place, proportionate to an overview; deep home is ch07.
- Staging pool: consensus mechanism (prose lists UE RHI, Vulkan/D3D12 primitives). Tier 2.

## Chapter 04 — Core Loop and Frame Lifecycle

Ch04 is the strongest comparative chapter of the four; its forks need touch-ups, not restructuring.

### CH04-F1: Pipeline depth — two-stage vs. UE's three-stage vs. Unity's models

- **Where:** "The Pipeline Concept".
- **Decision:** two-stage overlapped pipeline (Render Thread does both command building and submission), lockstep mode as opt-out.
- **Current prose:** compares UE5 3-thread (cited) and ND fibers (cited), argues the pick (wgpu abstracts API submission). Unity absent: main-thread + Graphics Jobs / RenderThread modes `[verify]`.
- **Verdict:** already-compared-with-rationale, minor gap. **Effort:** small — one Unity line (~60-100 words).

### CH04-F2: Interpolation vs. extrapolation

- **Where:** callout "Extrapolation vs. Interpolation".
- **Decision:** interpolation; rationale (overshoot artifacts) present.
- **Current prose:** "Some engines extrapolate" — no engine named. Which engines actually ship extrapolation, and where (e.g., Source's networked-entity handling, physics prediction modes) `[verify — the claim is currently unfalsifiable]`.
- **Verdict:** compared-with-rationale but engine-anonymous. **Effort:** small, ~50-100 words: name and verify the actual engines/modes.

### CH04-F3: Device loss — fatal-with-grace vs. recovery walk

- **Where:** "Device Loss and the 'GPU Is a Cache' Invariant".
- **Current prose:** both options presented with trade-offs, UE/Unity noted as implementing recovery, cert requirements mentioned. **Gap: Smallworld's own pick is never stated** — the prose says "the pragmatic starting point for any engine" but stops short of committing.
- **Verdict:** compared, rationale present, pick implicit. **Effort:** small, ~50-80 words: state the v1 pick explicitly (check `docs/architecture/capability-tiers.md` for the decided tier) and the trigger for graduating to the recovery walk.

### CH04-F4 (optional): Render-to-game feedback channel

- **Where:** "Render-to-Game Feedback".
- **Decision:** dedicated async feedback channel + engine/game quality-policy split.
- **Engines:** no engine ships a comparable formalized channel; partial precedents are telemetry APIs (Unity `FrameTimingManager`, UE stat system/CSV profiler `[verify]`).
- **Verdict:** closer to "Smallworld formalizes an informal practice" than a true fork — **recommend treating as Tier 2/invention**; optionally add a two-sentence precedent note. Effort: small (~80-120 words) if taken.

### Ch04 rejected borderlines

- Three clocks / fixed timestep / accumulator: consensus, fully grounded (Fiedler, Gregory, per-engine API mapping). Exemplary Tier-2 treatment.
- Fixed phases vs. event-driven scheduling: compared (Ogre3D negative example, Unity DOTS system groups) with rationale and cost. Done.
- Lockstep mode: UE `r.OneFrameThreadLag` comparison present.
- DRS as control loop: engine survey present (XDK, UE, Unity SRP); controller design argued. Done.
- Predictive tick pacing: Reflex cited; opt-in rationale argued. Done.
- Control channel: convergence framing with three-engine survey. Tier 2.
- Vsync/present modes: API-level consensus mapping. Tier 2.
- Teardown protocol: Smallworld formalization; engines expose no comparable options surface. Tier 3/invention, rationale present.

## Chapter 01 — Engine Renaissance

Survey/thesis chapter; sweep confirms most forks are either fully treated or owned by later chapters.

### CH01-F1: `wgpu` as the graphics abstraction vs. an in-house RHI

- **Where:** "Smallworld's Starting Point" → "The Universal Graphics Abstraction" (~line 233-245).
- **Decision:** adopt `wgpu`/WebGPU as the RHI-equivalent rather than building per-API backends.
- **Engines:** UE RHI described in detail with citations; alternatives acknowledged only generically. Unity's platform graphics abstraction and Godot's `RenderingDevice` absent `[verify both]`.
- **Verdict:** already-compared-with-rationale, but one engine of three. **Effort:** small, ~100-150 words: add Unity/Godot abstraction lines so the "every engine needs this layer" claim rests on the full survey.

### CH01-F2: Hybrid voxel+mesh unified rendering, raster-primary with RT reserved for effects

- **Where:** "Is the Game Engine a Solved Problem?" → "The value of a concrete instance" (~line 213).
- **Decision:** voxel volumes and triangle meshes share one rendering/lighting model; rasterization primary, raytracing for shadows/GI.
- **Engines:** prose silent — yet this is a genuine divergence (UE Nanite/Lumen, Unity URP/HDRP split with optional RT, Godot SDFGI/VoxelGI) `[verify all]`.
- **Verdict:** not-compared. **However:** the deep treatment belongs to the unwritten rendering chapters (12-16). **Action:** no ch01 retrofit; record as a **must-treat obligation for ch12/ch15 when written** (the tiered policy applies at writing time). Ch01's single sentence is acceptable as a thesis preview.

### Ch01 rejected borderlines (subagent + lead concurrence)

- Rust vs. C++: fully treated; the industry does not diverge on this axis (all three majors are C++ cores), so the fork is Smallworld-vs-industry and the existing two-sided argument with scoping limits satisfies the policy.
- `FramePacket` firewall preview: the chapter names the divergent engine strategies (proxy-per-primitive, server singleton, chunk batching) in its premise section but defers the argument; deep home is ch03/ch12 (see CH03-F3). Optional one-sentence connective pointer; no comparison owed in ch01.
- ECS spawn preview: flattened into a convergence claim here; deep home ch03 where it is fully treated. Optional wording nit only.
- Adopt/Customize/Build, licensing survey, scene-graph and physics open questions, platform-layer consensus, game-loop history, AI-assisted development, batteries-included spectrum: all fail condition (a) or (b) of the rubric. No action.
- **Internal-consistency nit (not a fork):** review question 7 asks about "GPU memory as a cache," a concept ch01's body never introduces (it is ch04 material). Fix the question or add a forward pointer at ch01's next revision.

## Chapter 02 — Engine Architecture at a Glance

Overview chapter: the sweep surfaced 13 candidates, but for most of them a later chapter owns the deep treatment, and the policy says the retrofit lands where the full discussion lives. Only two forks are ch02's own.

### CH02-F1: Boundary enforcement mechanism (compile-time crate structure)

- **Where:** "Layers Become Enforceable Boundaries" (~lines 93-97).
- **Decision:** layering enforced structurally — public API crate re-exports allowed types, render crate is a sibling of the game crate, violations are compile errors.
- **Engines:** UE build-system module graph rejects cycles (present); Godot server encapsulation via RIDs (present); Unity absent (assembly definitions/asmdef) `[verify]`.
- **Verdict:** compared-but-no-rationale ("the mechanism differs, but the principle is the same" — no argument for why compile-time crate boundaries win). **Effort:** small, ~100-150 words: add the Unity line and a per-alternative rationale sentence.

### CH02-F2: Explicit linear dependency-ordered boot

- **Where:** "Boot Once" (~lines 121, 138-140).
- **Decision:** single linear, readable boot order with aggregated feature declarations; lazy init rejected with rationale.
- **Engines:** UE topological module init (present); Godot independent servers (present); Unity absent (largely implicit/opaque subsystem init) `[verify]`.
- **Verdict:** already-compared-with-rationale, one engine missing. **Effort:** tiny, ~40-60 words.

### Ch02 candidates routed to their owning chapters

| Candidate (ch02 preview) | Owning chapter | Status |
|---|---|---|
| ECS object model | ch03 (done) + ch08 (unwritten) | ch03 fully treats; **ch08 obligation** below |
| Hybrid data model: ECS for game state, side structures for engine internals | ch08 (unwritten) | **ch08 obligation** — genuine fork (Unity DOTS Entities Graphics routes render data through entities vs. UE/Godot side structures `[verify]`); Smallworld's pick is backed by the sw-cf6350 benchmarks, which the chapter should cite as evidence |
| Two-thread pipelined loop | ch03/ch04 | captured as CH03-F1 + CH04-F1 |
| Fixed timestep | ch04 | see CH04-F5 (added below) |
| FramePacket firewall | ch03/ch12 | captured as CH03-F3 |
| Handle lifetime | ch06 | captured as CH03-F2 (lands in ch06) |
| Named budgets | ch06 (written) | **ch06 check item**: verify the budget section names engine precedents (UE scalability/pool CVars vs. Unity implicit vs. Godot minimal `[verify]`); likely Tier-2 formalization framing suffices |
| Polled input snapshot | ch05 | done (this cycle) |
| wgpu abstraction | ch01/ch12+ | captured as CH01-F1 |
| Plugin/extension architecture | **unassigned** | outline decision below |
| Code-first game description (App trait vs. editor-project-first) | **unassigned** | outline decision below |

### Ch02 rejected borderlines (subagent + lead concurrence)

Inversion of control (all majors identical); layered architecture itself (consensus; the fork is enforcement, captured as CH02-F1); shared foundation layer; `GameContext` explicit-context vs. ambient globals (Smallworld-vs-industry, majors converge on ambient — existing argument suffices); feedback events; retained render scene; capability discovery at boot; Rust-vs-C++ (see ch01); engine-lineage history.

### Addendum: CH04-F5 (from the ch02 sweep's fixed-timestep note)

Ch04's survey presents the fixed timestep as universal, but Unreal's default gameplay tick is variable-delta with optional physics substepping — a genuine divergence the survey currently papers over `[verify]`. One honest sentence in ch04's "Three Clocks" survey (~50-80 words). Small.

## Totals and execution recommendation

**In-place retrofits on written chapters (11 items, ~1,000-1,400 added words total):**

| Item | Chapter | Size | Research needed |
|---|---|---|---|
| CH03-F1 threading model + promised task-graph-evolution argument | 03 | **moderate** (~300-400 w) — the only ceiling-scale item | UE TaskGraph, Unity Jobs/Graphics Jobs, Frostbite/id |
| CH03-F3 extraction: Unity line | 03 | small | Unity SRP/RenderGraph |
| CH03-F2 handle lifetime: GC comparison | 06 (rider) | small-moderate | UE GC, Unity GC, Godot RefCounted |
| CH04-F1 pipeline: Unity line | 04 | small | Unity Graphics Jobs modes |
| CH04-F2 extrapolation: name the engines | 04 | small | Source engine et al. |
| CH04-F3 device loss: state the pick | 04 | small | none (check capability-tiers.md) |
| CH04-F5 UE variable-tick honesty | 04 | small | UE tick/substepping |
| CH04-F4 feedback precedents | 04 | small, **optional** | Unity FrameTimingManager, UE stats |
| CH01-F1 wgpu: Unity + Godot lines | 01 | small | Unity gfx abstraction, Godot RenderingDevice |
| CH02-F1 boundary enforcement rationale + Unity | 02 | small | Unity asmdef |
| CH02-F2 boot: Unity line | 02 | tiny | Unity subsystem init |

**Suggested execution:** three focused Phase-2 sessions riding the next revision pass of each chapter — (1) ch03 (F1+F3; the moderate one), (2) ch04 (F1/F2/F3/F5, optionally F4), (3) ch01+ch02 (CH01-F1, CH02-F1, CH02-F2) — plus the ch06 rider (CH03-F2 + budget check) whenever ch06 is next revised. Every `[verify]` must be vendor-doc-checked during its session, per policy.

**Obligations for unwritten chapters** (the policy applies at writing time; record so the writer agent inherits them): ch08 — ECS object-model full comparison AND the hybrid ECS-vs-side-structures fork with sw-cf6350 benchmark evidence; ch12/ch15 — voxel+mesh unified rendering thesis (CH01-F2) and extraction-mechanism depth; plus the two unassigned items below.

**Outline decisions — DECIDED by the author, 2026-08-19:**
1. **Plugin/extension architecture → a dedicated chapter.** (UE modules with broad API reach vs. Unity packages vs. Godot GDExtension; Smallworld's narrow per-role registration seams; matters doubly because voxels ship as the first-class Voxel Plugin.) Chapter number/position TBD in a future outline pass, since numbering intersects the in-flight ch22-26 restructure. Tracked in xpo.
2. **Code-first game description → ch09**, with ch02 keeping the preview. Winner argument to lean on the industry trajectory: Godot removed VisualScript in 4.0 and UE6 is dropping Blueprints, validating code-first as the durable direction. Obligation note planted in the ch09 stub.

**Internal-consistency nits found along the way** (not forks; fix at each chapter's next pass): ch01 review question 7 references "GPU memory as a cache," which the ch01 body never introduces; ch03's learn-bullet promises the task-graph-evolution argument the body doesn't deliver (folded into CH03-F1).
