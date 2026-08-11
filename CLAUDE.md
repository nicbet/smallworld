# Agent Instructions

This project uses `xpo` (Exponential) via the MCP server registered in `.mcp.json`.
Always use the MCP tools — never shell out to the `xpo` CLI.

Issue IDs in this project use the prefix `sw-` (e.g. `sw-a1b2c3`).

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

Two separate questions, two separate answers (clarified 2026-08-11 — earlier versions of this
section conflated them):

1. **Game Object Model → ECS.** The game-facing `World`: entities are IDs, components are plain
   data in per-type dense stores, systems are functions over queries. The real decision here was
   Object-Oriented vs. ECS, and ECS is the modern consensus. Spec: `docs/architecture-design.md`
   (Composability & Scripting). hecs 0.11 is the vetted crate.
2. **Engine internals → NOT ECS.** Render thread, GPU resources, streaming: side structs and
   dense pools (`GpuContext`, `SlotMap` resource pools, retained `RenderScene`). These are
   dense-iteration / spatial-traversal workloads where plain arrays win. The sw-cf6350
   benchmarks evaluated ECS as a general engine-internals mechanism and support *this* answer —
   do not cite them against the Game Object Model.

Transitional: `SlotMap<EntityId, Instance>` still bridges the OOC pipeline until the game-layer
World lands; new code targets the component model in `docs/architecture-design.md`.

**Performance rules (ECS usage):**

1. **Runtime state → mutable field.** `LodLevel(u8)`, `Visible(bool)`, `StreamingState(enum)`
   change per-frame — these are fields inside components, NEVER modeled as component
   presence/absence. Archetype migration is 500-660× slower than field mutation.
2. **GPU handles → side struct.** `Device`, `Queue`, pipelines, buffer pools stay in engine-side
   structs; game code holds opaque handles only.
3. **Batch spawns at load time.** Entity insert has overhead. Spawn during preload/streaming,
   never in the render loop.
