# Exponential Agent Instructions

This project uses `xpo` (Exponential) via the MCP server registered in `.mcp.json`.
Always use the MCP tools — never shell out to the `xpo` CLI.

Issue IDs in this project use the prefix `projects-` (e.g. `projects-a1b2c3`).

## Hard Rules

1. Every code change must be backed by an xpo issue transitioned to DOING before any file is modified.
2. Never start work on a BACKLOG issue without explicit user approval to transition it.
3. Bugs discovered during implementation may be filed and fixed without approval — file the issue, link it to the current work, and fix it.
4. Before beginning any implementation task, load the `xpo-workflow` skill and follow it.
5. If an MCP tool call fails, report the error to the user. Never fall back to the CLI.

## Agent Identity

Set the `assignee` field to yourself when transitioning an issue to DOING. Use the form
`<Agent Name> <agent@<host>.local>` — e.g. `Claude Code <agent@macbook.local>`.

## Entity Architecture

The engine uses **SlotMap + side structs** for entity storage. ECS (hecs) is deferred until
entity heterogeneity justifies it (game layer, material diversity). See sw-cf6350 for the full
benchmark data behind this decision.

**Current pattern:**
- `SlotMap<EntityId, Instance>` for instanced voxel objects (stable handles, O(1) insert/remove)
- GPU singletons (GpuContext, BrickPool, SVO, Raymarcher) as side structs
- Camera as a standalone struct
- BVH rebuilt from SlotMap iteration

**When to introduce ECS:**
ECS subset queries are 4-7× faster than scanning `Vec<Option>` — but only matter when entity
types diverge (different component sets per entity). Current workloads (culling, LOD, streaming,
GPU upload) are all dense iteration or spatial traversal where plain arrays win. Introduce hecs
when we need queries like "all entities with X but not Y" at scale.

**Performance rules (apply to both SlotMap fields and future ECS components):**
1. **Runtime state → mutable field.** `LodLevel(u8)`, `Visible(bool)`, `StreamingState(enum)`
   change per-frame. If ECS is adopted later, these must be fields inside components, NOT
   modeled as component presence/absence. ECS archetype migration is 500-660× slower than
   field mutation.
2. **GPU handles → side struct.** `Device`, `Queue`, pipelines, buffer pools stay in `GpuContext`.
3. **Batch spawns at load time.** Entity insert has overhead. Spawn during preload/streaming,
   never in the render loop.
