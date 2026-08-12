# Game ↔ Engine — Ownership, Handoff, and the Frame

The narrative view the structural diagrams (01–06) don't tell. Four questions, four diagrams.

## 1. Who holds what — and the one-time handoff

**A game holds almost nothing**: its `App` implementation (its own logic and state), *handles*
(`EntityId`, `AssetHandle<T>`, `ResourceHandle<T>`, `SoundHandle`), the `EngineConfig` it builds
once, and its data assets. Everything else is **engine-owned**. The game hands over exactly two
things, exactly once — `Engine::run(config, app)` — and from that moment **control is
inverted**: the engine calls the game (`init`/`fixed_update`/`update`) and *lends* engine-owned
state per call through `GameContext` borrows. There is no per-frame "passing of the World to
the engine" — the World never left.

@import "07-ownership.mmd" {as="mermaid"}

## 2. What the game loop does

Boot once, then two concurrent phase chains: the game thread runs frame N while the render
thread draws frame N−1 (the 2-frame pipeline), and the audio thread mixes continuously.

@import "07-game-loop.mmd" {as="mermaid"}

## 3. How a frame (and sound) gets made — the data's journey

One-way traffic in owned values: World changes become a delta packet; the retained scene
absorbs it; views cull it; the graph draws it; post shapes it; the swapchain shows it. Audio
commands take the short road. Feedback returns as advice, never as truth.

@import "07-frame-data-flow.mmd" {as="mermaid"}

## 4. Scene changes & pause (OQ 33)

The flow state machine — MainMenu → Loading → Playing → Paused — is **game code**; the engine
provides the primitives it composes. Pause freezes exactly one of the two clocks (the fixed
accumulator) while `update` keeps running on `real_dt`, so menus animate over a frozen world.
A world swap is built in the background while the current World renders (a loading screen *is*
the current World), then applied at a frame boundary as a **scene-reset delta**; assets survive
via refcounts, commands-audio outlives worlds, emitters die with their entities.

@import "07-world-swap.mmd" {as="mermaid"}
