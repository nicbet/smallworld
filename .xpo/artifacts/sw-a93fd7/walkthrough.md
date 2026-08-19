# Walkthrough: Ch05 internal-pass fixes + spec sync

## What was built and why

Chapter 05 (Input and Platform Interaction) went through an internal review pass (`book/reviews/2026-08-19-ch05.md`) that found the revision draft internally contradictory: listings disagreed with prose, both occasionally disagreed with `docs/architecture/describing-a-game.md`, five factual claims were soft, edge-case semantics were unspecified, and four arguments were duplicated. This issue applied all findings and, where teaching the material exposed real architecture defects, changed the spec too. Over four review rounds with the user it also produced two durable authorship policies and a forward-thread into Chapters 21/22.

## The design changes and how they fit together

**Context-stack ops live on `GameContext`, not `Input`.** The chapter's prose had quietly arrived at the right design while the listing and spec had `push_map`/`pop_map` on `impl Input` with `&mut self` — unreachable through `GameContext`'s immutable `input` borrow. Listings and spec now agree: the `Input` snapshot is never mutated by game code; stack mutation routes through `GameContext` methods backed by engine state.

**Tick-stamped edges replace the consumed-once latch.** The spec's `fixed_action_pressed` "consumed-once latch" contradicted the chapter's own promise that no input query has side effects, and made correctness depend on which system read first. The replacement: during INPUT, each action edge is stamped with the index of the *next fixed tick to run*; the query is true iff the executing tick's index equals the stamp. Zero-tick frames defer the edge (counter doesn't advance), multi-tick frames fire it once, reads are pure, and any number of consumers see the same answer. Only the fixed-tick half of Godot's dual stamp is needed because per-frame edges are recomputed each frame by the action-layer diff. Edges are stamped only for actions active on the context stack (no phantom edges after closing a menu); a pending stamp fires on the first tick after resume, which is one more reason pause should push a blocking map.

**The engine comparison behind it** (all vendor-doc verified): Unity re-clocks the whole input system via its update-mode setting (global; fixes FIXED by breaking UPDATE under our dual-clock phase model; Unity's changelog records both failure modes as bugs in that mode). Unreal avoids the mismatch structurally (no fixed-rate gameplay tick) and ships `UAsyncPhysicsInputComponent` to marshal input by value into UE5's async physics tick (thread boundary + networked prediction — a harder problem than our single-threaded tick has). Godot stamps edges per clock and makes the query clock-aware — the only strategy correct in both loops with no global mode and no data movement. Smallworld picks Godot's, and the chapter argues the pick per-alternative.

**Networking migration path (rounds 3-4).** Networked prediction/rollback needs input as per-tick serializable values. The tick stamp already defines which tick every edge belongs to, so the per-tick record (`TickInput` = resolved action values + edges stamped T) falls out mechanically; the query API survives with a swapped backing store (live stamps vs. recorded `TickInput` during re-simulation). Ch21's stub now materializes this; Ch22's stub showcases the backing-store swap in rollback, and its recording format was corrected from `(frame_index, Input)` to `(frame_index, game_delta, Input)` — without the per-frame delta, replay cannot reproduce the accumulator's tick cadence, so stamps (and every `fixed_action_pressed` result) could diverge. That was a live determinism bug in the stub.

**Other design fixes:** controllers are `Vec<Option<ControllerState>>` connection slots (holes keep indices stable within a session; steady state allocates nothing — the only design where the stable-index claim, the quarantine listing, and the perf rules all hold); sticks bind via new `Binding::ControllerStick` with a radial, direction-preserving dead zone while `ControllerAxis` stays scalar (per UE Radial/Axial modifier, Unity StickDeadzone/AxisDeadzone, Godot get_vector); multi-binding axis resolution is largest-magnitude-per-frame (Unity's disambiguation; deterministic, no cross-frame history); action edges are computed at the action layer by diffing resolved values (controller buttons need no device-level edge state; multi-bound actions edge once).

## Factual corrections

Sekiro (2019) carries the Xbox-prompts-on-keyboard story (previously misattributed to Elden Ring); the unsupported Cyberpunk input-latency claim was dropped; `UPlayerInput` replaces the nonexistent `FInputState`; USB-IF assigns vendor IDs while vendors assign product IDs; CVAA is scoped to accessible communication features (remapping remains a platform-cert expectation); composite vectors are "clamped" to unit length (opposing keys cancel; normalizing a zero vector is undefined).

## Policies codified (in `.claude/skills/book/SKILL.md`)

1. **Tiered engine comparison:** full options-comparison + per-alternative winner rationale only where engines genuinely *diverge* and Smallworld picks (ceiling: the Tick-Stamped Edges section); consensus material gets historical grounding only; micro-decisions get a sentence. "Consensus" or "first one that looked good" are never acceptable rationales.
2. **Word target is a soft signal:** 8-10k words flags runaway writing; depth is never cut to hit it; a lean chapter far over target signals a chapter split, raised with the author. (This chapter closed at ~14.5k raw words with the user's explicit acceptance — comparison depth was the priority.)

Retrofit of ch01-04 Tier-1 divergence passages is tracked in sw-438537 (audit first, then retrofit riding along each chapter's revision pass).

## Non-obvious notes for future readers

- Work happened in the **main checkout**, not an xpo worktree: the chapter baseline (321 lines) and spec edits were uncommitted there, so a worktree off the default branch would have been stale. Single active issue; no concurrency risk.
- Citations draw only from existing `references.bib` entries by user policy (`gregory2026game`, `gaffer-fix-timestep`, `ford-gdc2017`); writer agents should not add new bib entries during revision passes.
- Consciously skipped (flagged awareness-only in the review): the input-frame-timing figure's phase strip abbreviates and omits CLEAR; the "locked to the window center" cursor-capture phrasing is an implementation simplification of winit's Locked mode.
- Validation: Quarto HTML render of the chapter is clean (full-book PDF render is the normal build path; single-chapter PDF fails on `\input{figures/...}` path resolution by design). All listing/figure/citation cross-references verified by script.