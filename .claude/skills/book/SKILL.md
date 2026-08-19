---
name: book
description: >-
  Write or revise a chapter of "The Modern Game Engine" textbook.
  Use when asked to write, draft, or rewrite a book chapter.
metadata:
  author: nicbet
  version: "1.0"
---

# Chapter Writing Skill

Write a chapter for "The Modern Game Engine: Architecture, Systems and Principles"
by Nicolas Bettenburg, PhD. The book covers the Smallworld engine, a game engine
designed in Rust on top of wgpu.

## Before writing

1. **Read the chapter stub** at `book/chapters/NN-chapter-name.qmd` for the outline.
2. **Read `book/support/chapter-blueprint.md`** for the structural template.
3. **Read the preceding chapter** to understand where this one picks up.
4. **Read the next chapter's stub** to understand the forward transition.
5. **Read relevant `docs/architecture/*.md`** files for spec content to incorporate.
6. **Read `book/references.bib`** so you can cite existing entries and know what to add.
7. **Read `references/quarto-format.md`** for format conventions and examples.
8. **Read `references/tikz-figures.md`** for figure conventions.

## Core thesis

Decades of game engine lessons, keep the proven winners, discard the rest, apply
Rust and wgpu for a clean 2026 starting point. `docs/architecture.md` is the spec;
the book is the architectural textbook around it. Explain the *why* behind each
decision, compare industry approaches, and use Smallworld as the concrete example.

## Prose style

### Voice and register

- First-person plural ("we") and direct address.
- Meaty, verbose paragraphs. Each section should have substantial discussion.
- Authoritative but not dogmatic: present the reasoning, not just the conclusion.
- **NO emojis.**
- **NO em-dashes** (no `---`, no `—`). Link clauses naturally with colons, commas, semicolons, or restructure the sentence.
- **NO roadmap language.** The book describes end-point architecture in timeless
  terms; implementation staging lives in `docs/architecture/capability-tiers.md`
  as explicit tier marks, never in chapter prose. Do not write "planned",
  "v1/v2", "future refinement", "starting architecture", or "for now". Where
  alternatives really are staged, argue the designed end-point on its merits
  and present the alternatives as points on one architecture or spectrum.

### Didactic pyramid

Each section follows a top-down structure that moves from context to detail:

1. **Situation.** Open with the problem or design question in terms the reader
   already understands. Why does this matter? What goes wrong without it?
2. **General idea.** Present the principle, pattern, or architectural decision at
   a conceptual level before any implementation detail.
3. **Increasing detail.** Deepen through subsections: concrete data structures,
   code listings, edge cases, optimizations. Figures and listings earn their
   place here, where they aid understanding, not as decoration. When
   referencing a figure or listing, always follow with narrative that walks
   the reader through what they are looking at: name the key elements,
   explain what each part represents, and connect the visual back to the
   argument. Never drop a figure reference without description.
4. **Synthesis.** Close the section by connecting back to the broader architecture
   or contrasting with the alternatives that were ruled out.

### Historical grounding and comparative analysis

Architecture decisions do not exist in a vacuum. Help the reader understand where
each idea comes from and why it endures:

- **Show historical lineage.** Explain how mature engines arrived at a pattern:
  what problem surfaced first, what early solutions looked like, and what
  survived after decades of shipped games. Anecdotes from real engines (id Tech,
  Unreal, Unity, Godot, Frostbite, CryEngine) give decisions weight that an
  abstract argument cannot.
- **Compare engine-offered solutions, then argue the winner on merits.** Scale
  the treatment to how much the engines actually disagree:
  1. *Engines diverge and Smallworld picks one (or invents its own):* full
     treatment. Present each engine's strategy, verified against vendor docs
     rather than memory, then argue explicitly why Smallworld's pick wins over
     each alternative, on the merits, in Smallworld's context. "Consensus,"
     "first one that looked good," or an unexamined alternative are never
     acceptable rationales: Smallworld cherry-picks the absolute best ideas,
     and the prose must show the comparison that earned the pick. (Ceiling
     example: Chapter 5's Tick-Stamped Edges section.)
  2. *Engines converge and Smallworld follows:* a consensus statement plus
     historical grounding for why the pattern won. The industry already ran
     the tournament; do not force a per-alternative rationale onto settled
     questions.
  3. *Internal micro-decisions with no engine-visible precedent* (field
     layouts, container choices, naming): a sentence of local rationale; no
     comparison owed.
- **Distinguish winners from compromises.** Some patterns recur because they are
  genuinely best-in-class (ECS for game objects, extract-and-render firewalls,
  fixed timesteps). Others recur because an older constraint forced them and
  inertia carried them forward. Name which is which.
- **Explain pitfalls through consequences.** When a design choice prevents a
  failure mode, describe the failure concretely: what breaks, what the player or
  developer experiences, and why the alternative architecture avoids it.
- **Compare fairly.** When referencing UE5, Unity, or Godot, describe their
  actual design, not a caricature. Acknowledge strengths before explaining where
  Smallworld diverges and why.

### Reader takeaways

Every section should leave the reader able to do two things:

1. **Reason from constraints to architecture.** Given a new subsystem or a
   different engine, apply the same principles to arrive at a sound design.
2. **Evaluate trade-offs.** Understand what was gained, what was given up, and
   under what conditions the decision should be revisited.

## Heading discipline

A heading earns its place only when it divides its parent into two or more peer
ideas. Never create a lone subsection. If a single point needs emphasis but not a
TOC entry, use a bold lead-in in the prose.

## Target length

8,000-10,000 words per chapter (30-40k characters) is a reference point, not a
cap. It exists to flag runaway writing; different chapters legitimately need
different budgets depending on the depth of their material, and a chapter dense
with genuine engine-divergence comparisons runs longer than a survey chapter,
and should. Never cut comparison depth, winner rationales, or industry
grounding to hit the number. Treat significant overshoot as a soft signal to
investigate: first hunt for redundancy and duplicated arguments; if the chapter
is lean and still far over, that is evidence it wants to split into two
chapters, which is an outline decision to raise with the author, never a trim
to perform silently.

## Verification checklist

After writing, verify:

- [ ] Every `#lst-name` has a matching `@lst-name` in the prose.
- [ ] Every `\input{figures/...}` file exists in `book/figures/`.
- [ ] Every `\ref{fig:...}` has a matching `\label{fig:...}` in the TikZ file.
- [ ] Every `[@citation-key]` exists in `book/references.bib`.
- [ ] No em-dashes (`---` or `—`) anywhere in the text.
- [ ] No roadmap language (planned / v1 / v2 / future / for now); staging belongs in `capability-tiers.md`.
- [ ] No emojis.
- [ ] Opening connects to the previous chapter.
- [ ] Summary tees up the next chapter.
