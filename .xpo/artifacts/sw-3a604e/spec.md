# Spec: Technical Design Document

## What

Write `docs/DESIGN.md`: the durable record of the architecture designed in the 2026-08-05 design discussion, covering vision, stack decision, data model, rendering, editing, streaming, destruction, lighting, worldgen, water & atmosphere, persistence, milestone ladder (mapped to the filed epics), and risks.

## Why

The design currently lives in a conversation and in `notes.md` (which is analysis of a *prior* Godot experiment, not a design). Every future issue in the board references concepts (brick pool, op log, TLAS, anchors, hot/cold, active-sim set) that need one canonical definition.

## How

Single Markdown document. Structure: Vision → Decisions (with rationale and revisit-conditions) → Architecture by subsystem → Nanite/Lumen mapping → Milestones (linked to epic IDs) → Risks/deferred knobs → References. Numbers (scale, brick size, SSE distances, budgets) computed for the decided 10 cm scale.

## Acceptance criteria

- [ ] All decisions from the discussion captured with rationale, none silently changed
- [ ] Every filed epic appears in the milestone section with its ID
- [ ] Open questions and deferred knobs explicitly listed
- [ ] No content contradicts notes.md's measured conclusions (sub-pixel policy, crack-free LOD)
- [ ] Water/atmosphere: representation-level commitments live in early milestones (data model, generator, traversal); simulation deferred to dedicated epics (review round 1)

## User decisions (resolved before writing)

- **Stack: Rust + wgpu** — chosen by user from options (vs C++/Dawn, C++/Vulkan+MoltenVK)
- **Base voxel scale: 10 cm** (Teardown-class) — chosen by user over 2.5 cm and 1 cm; per-instance finer scale kept as upgrade path

## Review round 1 (2026-08-05)

User: water/fluids and weather are integral (rivers, oceans, waterfalls; fog, god rays, clouds, rain, wind) — account for them early, not post-M7.

Resolution — **representation early, simulation late** (new decisions D9/D10):
- Still water = ordinary voxel material: generated (water table in generator v0), DAG-compressible, persisted via existing paths. Reserved in the M1 data model.
- Raymarcher traversal loop structured so a hit can continue as a transmission segment (refraction/Beer–Lambert tint) even though implemented later.
- Flowing water promotes bricks into an active-sim set with auxiliary level/flow channel (same hot/cold pattern as editing) — dedicated Water epic.
- Fog/god rays = accumulation along the existing primary march; clouds/rain/wind in a dedicated Atmosphere & Weather epic; global weather state plumbed as uniforms from M0.
- Heat/temperature sims remain deferred.