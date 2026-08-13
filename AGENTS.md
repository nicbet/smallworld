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
   benchmarks evaluated ECS as a general engine-internals mechanism and support _this_ answer —
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

## Book

The core idea for this book and the smallworld engine is: consider decades of game engine lessons, keep all the things that genuinely worked and came out as real winners, discard everything else, apply modern language (Rust) and graphics API abstractions (wgpu) and you get a killer engine design for 2026 and forward.

The `docs/architecture.md` document is the engine’s technical specification: the decisions, boundaries, data models, and implementation direction.

The book’s job is to be the architectural textbook around it:

- Explain the industry lessons behind each decision.
- Distinguish durable winners from historical compromises.
- Show why Rust and wgpu let the design start from a cleaner baseline.
- Teach readers how to reason from constraints to architecture, rather than merely describe Smallworld’s code.
- Use Smallworld as the concrete, running example of a modern engine design.

That gives the book a clear thesis: preserve what decades of shipped engines proved valuable; discard accidental legacy; use modern systems tools to build an understandable, high-performance engine for 2026 onward.

### Rules

- Book edits may be done without following the xpo workflow.
- Chapters should follow the blueprint recorded in `book/support/chapter-blueprint`
