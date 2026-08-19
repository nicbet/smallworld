# Spec: Phase 1 audit — ch01-04 engine-comparison forks

## What

Sweep chapters 01-04 and enumerate every **Tier-1 passage** per the tiered comparison policy (`.claude/skills/book/SKILL.md`): a design point where major engines (UE, Unity, Godot at minimum; others where relevant) ship *divergent* implementations or guidance AND Smallworld picks one or invents its own. Phase 1 is analysis only — no chapter edits, no web research; engine-behavior notes come from working knowledge and are flagged where vendor-doc verification is required before Phase 2 writes prose.

## Deliverable

`book/reviews/2026-08-19-ch01-04-comparison-audit.md` containing, per fork: chapter/section anchor, the decision Smallworld makes, one line per engine on what it ships (with `[verify]` flags), whether current prose already carries any comparison/rationale, the tier verdict for borderline calls, and a per-fork Phase-2 effort estimate (comparison depth needed, expected added words, trim candidates in exchange). Close with a total estimate and a recommended execution order (ride along each chapter's next revision pass).

## Method

Ch01/ch02 candidate sweeps delegated to subagents with the Tier-1 rubric (expected near-zero forks; agents enumerate candidates with quotes, final classification stays with the main pass). Ch03/ch04 read directly (expected to hold the real forks). Tier-2 consensus passages and Tier-3 micro-decisions are recorded only when explicitly rejected as borderline, so Phase 2 knows they were considered.

## Acceptance criteria

- [ ] Every ## / ### section of ch01-04 considered
- [ ] Each fork has per-engine notes with verification flags
- [ ] Borderline rejections documented with tier rationale
- [ ] Per-fork and total Phase-2 estimates present
- [ ] No chapter files modified in Phase 1

## Workflow note

Running in the issue worktree (chapters 01-04 are fully committed; the audit writes one new file in `book/reviews/`). Merge only after user tophat.