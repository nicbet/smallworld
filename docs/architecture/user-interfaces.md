## UI Framework (Stance)

_(OQ 18 amendment, 2026-08-11. The dev-tooling half of OQ 18 stands — egui. The game-facing half upgrades from "post-v1, own design round" to "architectural stance committed now, detailed design round later.")_

**UI widgets are entities.** This is the Godot lesson — its editor is built with its own UI framework, and UI nodes _are_ scene nodes — translated to ECS, with Bevy as the shape's Rust precedent (UI nodes as entities, flexbox layout via `taffy`). A separate Slate-style retained tree was rejected: that separation is why UE had to invent UMG as an authoring wrapper, and why UE UI composition never feels like scene composition. Unification is what makes Godot-fast composition possible — and here it compounds through existing systems at zero new machinery:

- **A menu is an OQ 20 document** — composed like a scene, saved like a scene, instantiated like a scene, diffable in RON.
- **Behaviors attach to UI entities** through the `BehaviorHost` — button logic in Lua, day one.
- **Widget events ride the double-buffered event bus.**
- **The `Animator` can drive UI properties** — the same data-graph machinery animates menus.
- **Themes are assets** — GUID-identified, hot-reloadable like everything else.
- **The entity hierarchy is the widget hierarchy** — layout-computed rects propagate through the same parent mechanism.

### Committed Now (so nothing precludes it)

- **Layout: a flexbox-class engine + anchor mode** — covering both of Godot's idioms (containers and free anchoring); `taffy` is the proven Rust implementation.
- **Rendering: a display-resolution render-graph pass after post-processing.** Game UI is never resolution-scaled by DRS; it sits below the dev-UI (egui) overlay.
- **Input priority & focus.** UI consumes input ahead of game systems — a routing layer with focus/capture semantics in the Input path.
- **UI scenes, themes, and styles are data** (OQ 20 documents and assets), never code-only.

### Deferred to the UI Design Round

The widget catalog, theming details, text shaping (cosmic-text-class), localization hooks, accessibility.

### The Editor Consequence

The editor is **an application built on the engine** — Godot's existence proof, adopted. It consumes the UI framework, the reflection layer behind the serialization registry (OQ 20), aux views, and the pick pass (OQ 22). It is not engine core, and it gets no design until there is a runtime to edit. The engine's only obligation is that its primitives suffice — the same API-sufficiency test the Voxel Plugin enforces for rendering, applied to tooling.
