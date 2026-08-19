# Spec: Ch05 internal-pass fixes + spec sync

## What

Apply the 2026-08-19 internal review (`book/reviews/2026-08-19-ch05.md`) to chapter 05 and sync `docs/architecture/describing-a-game.md` §5 (Input) where teaching the material uncovered architecture defects.

## Why

The chapter contradicts itself (listings vs. prose), contradicts the spec in places, contains five factual soft spots, is missing edge-semantics coverage and citations, and is ~2k words over target due to duplicated arguments.

## How

### Design changes (chapter listings + spec, per user decisions)

1. **Context stack ops → `GameContext`** (user decision). Remove `push_map`/`pop_map` from `impl Input` in `@lst-action-queries` and spec §5; add them to the `GameContext` impl in `@lst-game-context` and the spec's GameContext block. Rationale: the snapshot stays immutable; stack mutation routes through engine state.
2. **Fixed-tick edges → tick stamping** (replaces consumed-once latch). During INPUT, each action edge is stamped with the index of the next fixed tick; `fixed_action_pressed` is true iff the executing tick's index equals the stamp. Pure query (`&self` honest), any number of consumers, zero-tick defers, multi-tick fires once. Precedent: Godot's clock-aware `is_action_just_pressed` (stamps process-frame and physics-frame counters; verified against Godot docs/issues 2026-08-19). Chapter section retitled "Tick-Stamped Edges and the Intent Pattern"; manual intent pattern retained as the custom-semantics fallback. New paragraph covers: context stack applies at stamp time; pending stamp fires on first tick after resume (pause should push a blocking map); sub-frame edges collapse.
3. **Stick binding + radial dead zone**. New `Binding::ControllerStick { device, stick: StickSide (Left|Right), dead_zone }` yielding Axis2d with a radial dead zone (magnitude-based, direction-preserving rescale); `ControllerAxis` stays scalar with per-axis dead zone. Precedent: UE `UInputModifierDeadZone` Radial/Axial + 2D thumbstick keys (verified against Epic docs 2026-08-19); Unity StickControl + `StickDeadzone` vs `AxisDeadzone` processors; Godot `get_vector` circular dead zone. The "move" example rebinds to `ControllerStick`.

### Review round 2 (2026-08-19, user feedback)

**Authorial rule (durable, also added to the book skill):** when engines ship a concrete implementation or official guidance for a problem, the book must discuss and compare those options; when Smallworld follows one, the prose must explicitly argue why that design wins **on the merits against each alternative**. "We didn't research the others" or "first one that looked good" are never acceptable rationales — Smallworld cherry-picks the absolute best ideas, and the argument must show the comparison.

**Application to item 2 (tick-stamped edges):** the section's engine survey expands from "most engines leave it to game code + Godot" to the three real strategies, each verified against vendor docs:
- **Unity — re-clock the input system.** Input System update mode (Process Events in Dynamic vs. Fixed Update); fixed mode recommended for physics-driven games; global switch, so the variable-rate side inherits the mirror-image problem; changelog records both failure cases as bugs in fixed mode (ISXB-1006 double-true; same-frame press/release lost). Legacy Input Manager: documentation warning only.
- **Unreal — structural avoidance + marshaling.** No fixed-rate gameplay tick in the classic model (Enhanced Input delegates fire once per game-thread frame); for UE5's opt-in fixed-rate async physics tick, `UAsyncPhysicsInputComponent` captures input on the game thread and delivers it by value to the async tick (client + server) — the intent pattern productized as a command buffer for networked physics prediction.
- **Godot — clock-aware query.** Dual frame/tick stamping; same query correct in both loops; pure read.

**Winner rationale (merits, per alternative):** Unity's global re-clock is incompatible with Smallworld's phase model, which runs both clocks every tick (UPDATE consumes frame edges, FIXED consumes tick edges) — re-timing the whole system fixes one consumer by breaking the other. Unreal's marshaling solves a harder problem than Smallworld has (thread boundary + network prediction); buying it for a single-threaded game tick imports a queue and ownership handoff where a stamp comparison suffices (marshaling-by-value remains the escape hatch if fixed ticks ever move off-thread — the manual intent pattern is that shape). Godot's is the only strategy where the same query is correct in both loops with no global mode and no data movement, at a cost of two integers per action. Smallworld needs only the fixed-tick half of Godot's dual stamp because per-frame edges are recomputed each frame by the action-layer diff.

### Agent decisions (delegated: "informed by other engines")

- **Controller storage → `Vec<Option<ControllerState>>` (slot semantics)** in chapter and spec. Resolves the review's three-way contradiction (Option-indexing in the quarantine listing; "fixed-size array" leftover; per-frame rebuild vs. stable-index claim). Connect occupies first free slot; disconnect leaves a `None` hole so indices stay stable within a session; steady state allocates nothing. Matches XInput/engine slot conventions. Revisit if: we'd rather drop index stability entirely and address controllers by `ControllerId` only.
- **Multi-binding axis resolution → largest magnitude wins per frame** (replaces "most recent non-zero contributor + analog priority", which was two conflicting rules requiring cross-frame history). Unity's disambiguation-by-actuation-magnitude precedent. Deterministic pure function of the snapshot. Revisit if: per-device-class priority is ever wanted.
- **Action edges computed at the action layer** (diff of resolved action values frame-over-frame), covering controller buttons without device-level edge state. Closes the review's 1.4 gap without adding `prev` fields to `ControllerState`.

### Factual corrections

Sekiro (not Elden Ring) as the Xbox-prompts-on-keyboard example; drop the Cyberpunk input-latency claim; `UPlayerInput` accumulates key state (not "`FInputState`"); USB-IF assigns vendor IDs, vendors assign product IDs; CVAA scoped to accessible communication features (remapping stays a platform-cert expectation); composite vectors "clamped" not "normalized" to unit length.

### Missing content

Edge-semantics boundary conditions (see item 2); touch scoping sentence in Cross-Platform Policy; `CursorMoved` + connect/disconnect arms in the quarantine listing; auto-repeat harmlessness sentence; `&str`-hashing honesty note on interned action names. Citations from existing bib entries only (user decision): `gregory2026game` (quarantine survey), `gaffer-fix-timestep` (fixed-tick edge problem), `ford-gdc2017` (input-deterministic replay/Overwatch).

### Redundancy trims (primary home in parentheses)

`DeviceFilter` Any/Specific rationale (action-mapping section); replay argument (The Input Foundation for Replay); drift/dead-zone derivation (Analog Inputs for drift, dead-zone paragraph for mechanics); console-port-unchanged point (Platform Adapter closing paragraph). Plus mechanical: "leting" typo, missing quote, spaced-hyphen clause links, `---` em-dashes in `fixed-tick-edge-problem.tex` node labels, wrong `@sec` cross-reference, stale `ActionKind` comment, glyph wording, DeviceTracker per-frame update cadence sentence, `App` hook signatures aligned with spec (`&mut GameContext` + `dt`/`fixed_dt`).

## Acceptance criteria

- [ ] Chapter contains no listing/prose contradictions from the review (1.1-1.8)
- [ ] Spec §5 and GameContext block match the chapter's listings
- [ ] Five factual fixes applied
- [ ] Edge-semantics paragraph present
- [ ] Tick-stamp section compares all three engine strategies and argues the winner on merits per alternative
- [ ] ≥3 citations, all resolving to existing `references.bib` keys
- [ ] No `---`/`—` in chapter or its figures; no `" - "` clause links
- [ ] Word count: over target accepted by user in favor of comparison depth (surfaced 2026-08-19)
- [ ] All `@lst`/`\ref` cross-references still resolve

## Workflow note

Working in the main checkout (not a worktree): the chapter baseline (321 uncommitted lines) and spec edits live there uncommitted; a worktree off the default branch would be stale. Single active issue. No commit until user tophat.