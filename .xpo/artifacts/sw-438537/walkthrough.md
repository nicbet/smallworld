# Walkthrough: Ch01-04 engine-comparison audit (Phase 1)

## What was built and why

Phase 1 of applying the tiered engine-comparison policy (codified in `.claude/skills/book/SKILL.md` during sw-a93fd7) retroactively to chapters 01-04. The fear was that the policy would force a full restructure of five chapters; the audit's job was to replace that fear with a real number before committing to any retrofit work. Deliverable: `book/reviews/2026-08-19-ch01-04-comparison-audit.md`, plus obligation notes planted in four future-chapter stubs after the author's follow-up decisions.

## Method

Chapters 03 and 04 were read in full by the lead pass (they were expected to hold the real forks, and did). Chapters 01 and 02 were swept in parallel by two subagents given the Tier-1 rubric verbatim — a fork requires BOTH a Smallworld decision AND genuine engine divergence — with candidate lists returned for lead-pass final classification. This split kept judgment on the classification while parallelizing the reading. Engine-behavior notes are working-knowledge and every claim carries a `[verify]` flag; Phase 2 sessions must vendor-doc-verify before writing prose (the policy forbids arguing a winner against unverified alternatives).

## The result in one line

Eleven in-place retrofit items, exactly one moderate (CH03-F1, the threading-model fork, which also has a learn-bullet promising an argument the body never delivers), ten small (mostly "Unity is the missing engine" lines, an unnamed "some engines extrapolate" claim, and a device-loss section that presents both options but never states Smallworld's pick). Total ~1,000-1,400 added words across three retrofit sessions plus a ch06 rider — an order of magnitude below a restructure.

## Key classification calls a future reader should understand

- **Overview chapters route, they don't own.** Ch02 produced 13 candidates but only two ch02-owned retrofits (boundary enforcement, boot); the rest route to their deep-treatment chapters, some already satisfied (ch03's ECS treatment, ch05's input work), some becoming obligations on unwritten chapters.
- **Smallworld-vs-industry is not a fork.** Rust-vs-C++ and GameContext-vs-ambient-globals fail condition (b): the majors do not diverge among themselves. The existing two-sided arguments suffice.
- **Formalizations of informal practice are Tier 2.** The feedback channel, teardown protocol, and named budgets have no engine-shipped divergent options surface; they get precedent notes at most.
- **Unwritten chapters inherit obligations at writing time**, planted directly in their stubs (ch08: ECS model comparison + the hybrid ECS-vs-side-structures fork with sw-cf6350 benchmarks as evidence; ch09: the code-first fork; ch12: extraction mechanism + voxel/mesh unified domain; ch15: raster-first lighting ladder), using ch12's existing "Deferred from Chapter 3" bullet convention so writer agents can't miss them.

## Author decisions folded in

Plugin/extension architecture gets a **dedicated chapter** (sw-e78dea, BACKLOG; numbering deferred because it intersects the in-flight ch22-26 restructure). Code-first game description lands in **ch09**, with the winner argument anchored on the industry trajectory (Godot removed VisualScript in 4.0; UE6 is dropping Blueprints — both flagged for verification at writing time).

## Follow-up structure

Four Phase-2 issues (sw-2f2873 ch03, sw-098dc0 ch04, sw-e1cbe3 ch01+02, sw-c65b57 ch06 rider) all depend_on this audit and are designed to ride each chapter's next scheduled revision pass rather than run as a standalone campaign. Nitpicks in sw-a89cf8. Each issue description is self-contained enough to execute without re-reading the audit, but the audit remains the authority on borderline rejections — future passes should not re-litigate the rejected borderlines without new evidence.

## Non-obvious notes

- Phase 1 deliberately performed **no chapter edits** and **no web research** — enumerate and estimate only. The `[verify]` flags are the research work-list for Phase 2.
- This issue ran in a proper xpo worktree (unlike sw-a93fd7, which had an uncommitted-baseline constraint); chapters 01-04 were fully committed, so the worktree flow applied cleanly.