---
name: chapter-writing
description: >-
  Write or revise a chapter of "The Modern Game Engine" textbook.
  Use when asked to write, draft, or rewrite a book chapter.
metadata:
  author: nicbet
  version: "2.0"
---

# Chapter Writing Skill

Write a chapter for "The Modern Game Engine: Architecture, Systems and Principles"
by Nicolas Bettenburg, PhD.

## Before writing

1. **Read the chapter stub** at `book/chapters/NN-chapter-name.qmd` for the outline.
2. **Read `book/support/chapter-blueprint.md`** for the structural template.
3. **Read the preceding chapter** to understand where this one picks up.
4. **Read the next chapter's stub** to understand the forward transition.
5. **Read relevant `docs/architecture/*.md`** files for spec content to incorporate.
6. **Read `book/references.bib`** so you can cite existing entries and know what to add.
7. **Scan `book/figures/`** to see existing TikZ conventions.

## Book identity and argument

*The Modern Game Engine* is a book about architectural judgment. It does not attempt
to claim that existing game engine architecture principles have become obsolete. It
asks a more synthetic design question: **if we were designing a modern engine from
first principles today, which lessons from shipped engines would we keep, which
historical constraints could we avoid, and how would modern tools change the resulting
architecture?**

The industry has run decades of experiments across proprietary engines, id Tech,
Unreal, Unity, Godot, and many focused in-house engines. Some ideas have become
durable winners: explicit ownership, stable asset identity, fixed-step simulation
where determinism matters, data arranged for the work that consumes it, asynchronous
work with budgets, and tools that make performance and correctness observable. Other
choices were sensible responses to particular hardware, languages, APIs, or production
histories, but are not principles a new design needs to inherit. What remains
under-served is a single, coherent distillation of those lessons: one that starts
from first principles, distinguishes the durable from the accidental, and shows how
the pieces fit together in a modern game-engine design.

The book begins with the patterns the industry has already validated, rather than
with a blank slate. It explains why a game-facing World should be distinct from
renderer-owned state; why data should be organized around the work that consumes it;
why asset identity and residency should be explicit; why simulation, asynchronous
services, and rendering each need bounded work; and why diagnostics belong in the
architecture before a team needs them. It then asks what changes when those ideas
are expressed with modern systems tools.

**Position relative to Gregory.** Gregory's _Game Engine Architecture_ is the
definitive reference on engine shape. It is essential reading, and this book does not
attempt to replace it. Gregory surveys the domain; this book adjudicates from first
principles. Engines overlap in some areas and diverge in others, and no existing
reference systematically distinguishes which overlaps are best practices and which are
historical accidents. Each chapter of this book traces the design pressures behind a
domain, compares how mature engines resolved them, and identifies which resolutions
are durable principles and which are artifacts of a particular era. Where Gregory
provides the vocabulary and the map, this book provides the engineering practice and
the judgment.

## Smallworld's role

Smallworld is the book's running concrete instance. It is a game engine built from
scratch in Rust on `wgpu`, designed to inherit the industry's proven subsystem
boundaries while starting clean on current hardware and tooling. It is not a toy
renderer or a framework sketch; it is a hybrid engine where voxel volumes and
triangle meshes share the same rendering and lighting model.

**Argument direction: principle first, then realization.** Each chapter begins with
the general problem and examines how mature engines have solved it, then shows how
Smallworld resolves the same responsibility in its own stack. The argument always
runs in that direction. Never lead with "Smallworld does X"; lead with why the
problem exists and what the industry learned, then show how Smallworld realizes it.

**Stack choices are Smallworld's, not universal prescriptions.** Rust and `wgpu` are
Smallworld's deliberate choices that produce unusually clean engineering answers to
ownership, concurrency, and platform-abstraction problems. They are not presented as
the only viable path. Acknowledge that C++ engines ship outstanding games, that other
graphics abstractions are equally valid, and that the principles the reader learns are
transferable regardless of stack.

**Introduce Smallworld once.** Chapter 1 introduces Smallworld at the point where the
argument calls for a concrete instance. Later chapters reference it without
reintroduction. Do not re-explain what Smallworld is or what it is built on; the
reader already knows.

## Prose style

- First-person plural ("we") and direct address.
- Meaty, verbose paragraphs. Each section should have substantial discussion.
- Balanced: explain what approaches exist, why the chosen one wins, common pitfalls.
- Show how Rust and `wgpu` give Smallworld clean answers to friction in the engine shape.
- Back up claims with citations from web articles, scientific research (papers, journal articles, theses).
- **NO emojis.**
- **NO em-dashes** (no `---`, no `—`). Link clauses naturally with colons, commas, semicolons, or restructure the sentence.

### Ground arguments in real engines and shipped games

Every architectural claim should be supported by concrete evidence from the industry:
real engines, real games, real developers, and real technical decisions. This is not
decoration; it is the book's epistemology. The argument is "the industry learned X"
and the evidence is *who* learned it, *when*, and *what happened*.

- **Compare across engines.** When discussing a subsystem, show how UE5, Unity, and
  Godot each resolved it differently. Name the specific mechanism (Unreal's
  `FSceneProxy`, Godot's `RenderingServer`, Unity's DOTS archetype chunks), explain
  the design pressure behind each, and evaluate which aspects are durable principles
  versus era-specific artifacts.
- **Use shipped games as evidence.** _Manor Lords_ demonstrating that DX is a force
  multiplier, _DOOM_ illustrating tight game-engine coupling, _Quake_ creating the
  licensing model, _Crysis_ pushing hardware limits. The game is the evidence; the
  architectural lesson is the point.
- **Name developers and their reasoning.** When a developer or team articulated *why*
  they made a particular choice (Styczeń on UE4's quality-of-life features, id
  Software on cycle-level optimization), cite their reasoning directly. First-hand
  technical rationale is stronger than general claims.
- **Draw on GDC talks, postmortems, and technical blogs.** These are primary sources
  for "why was it built that way?" Cite them. Scattered fragments become coherent
  evidence when assembled around a specific architectural question.
- **Include historical context that explains the present.** "Unity solved this that
  way because they had to support..." or "Unreal's object model traces back to
  UnrealScript and a single-core world." The reader should understand not just what
  exists but why it exists in that particular form.

### Argumentation techniques

The book uses several recurring techniques to make architectural arguments
concrete and persuasive. These are not optional flourishes; they are the
connective tissue that turns abstract principles into engineering judgment.

- **Failure-mode storytelling.** Build vivid narratives around what goes wrong
  without the principle. "Consider a small studio that builds its engine by
  bolting subsystems together..." or "The mesh is wrapped in a shared_ptr held
  by three different owners..." The reader should feel the pain of the failure
  mode before the principle that prevents it. These can be hypothetical
  scenarios, not just real shipped games.
- **Counterfactual reasoning.** State the principle, then show what breaks
  without it. "Consider the alternative" or "what happens without explicit
  feedback?" The counterfactual makes the cost of *not* following the principle
  tangible. Pair it with a code listing when possible (the library-model loop
  in Chapter 2 is the template).
- **Trace a datum across boundaries.** Follow one piece of data (a mesh, a
  sound, an input event) through its full journey across domain boundaries.
  Name each boundary, show how the representation changes, and explain what
  freedom each transformation buys. This makes abstract architecture diagrams
  concrete.
- **Cross-domain analogy.** When a principle is universal (inversion of control,
  handle-based ownership, explicit feedback), show it operating in a domain
  outside game engines: web frameworks, databases, operating systems, compiler
  design. This validates that the pattern is durable, not parochial.
- **Acknowledge costs.** Never pretend a recommendation is free. "The cost of
  maintaining these boundaries is not zero." State the cost, then explain why
  the benefit justifies it. This builds credibility and helps the reader make
  informed tradeoffs in their own work.
- **Forward and backward chapter references.** Weave a cross-reference web
  across the book. "Chapter 3 establishes why...", "Chapter 12 returns to this
  boundary as a concrete renderer design." The reader should always know where
  a concept is introduced, where it is developed, and where it pays off. This
  makes the book a coherent argument rather than independent essays.

### Code listings as argument

Every code listing must advance the architectural argument. A listing is not
"here is some code"; it is "here is the point, and this code is the evidence."
Introduce the listing with the specific claim it supports, show the code, then
analyze what the code reveals about ownership, boundaries, or costs. If a
listing does not serve an argument, cut it.

### No redundancy within a chapter

Make each argument once, in the right place. If a point was established in an
earlier section of the same chapter, reference it rather than restating it. Our
editing rounds found that restating the same argument in different words
inflates word count without adding insight, and the reader notices. When a
concept from a *different* chapter is needed, a brief forward or backward
reference is sufficient: "as Chapter 3 will establish" or "the firewall
introduced in Chapter 2."

## Heading discipline

A heading earns its place only when it divides its parent into two or more peer
ideas. Never create a lone subsection. If a single point needs emphasis but not a
TOC entry, use a bold lead-in in the prose.

## Code listings

- Annotate with `{#lst-name .rust lst-cap="Caption text"}`.
- **Every listing MUST be cross-referenced** in the prose before it appears, using `@lst-name`. Example: "as shown in @lst-foo:" followed by the code block. Never leave a listing unreferenced.

## Figures

- Create TikZ figures in `book/figures/` using the styles defined in `book/preamble.tex` (`engine-layer`, `engine-package`, `engine-boundary`).
- Reference with `\input{figures/filename.tex}` and `Figure \ref{fig:name}` in the prose.
- Every figure needs `\begin{figure}[!tb]`, `\caption{}`, `\label{fig:name}`, and `\FloatBarrier`.
- Use `>=Latex` arrow style, `\sffamily\scriptsize` fonts, `line width=0.4-0.6pt`.

## Bibliography

- Cite with `[@key]`.
- Add new entries to `book/references.bib` when needed (use `@online{...}` for web sources, `@book{...}` for books).
- Prefer existing bib entries when applicable.

## Callout boxes

```
::: {.callout-tip appearance="simple" title="Short Title"}
Content here.
:::

::: {.callout-warning appearance="simple" title="Short Title"}
Content here.
:::
```

Use sparingly: tips for best practices, warnings for common mistakes or pitfalls.

## Target length

Roughly 8,000-10,000 words per chapter (30-40k characters). Enough for a substantial
textbook chapter, not a survey article.

## Verification checklist

After writing, verify:

- [ ] Every `#lst-name` has a matching `@lst-name` in the prose.
- [ ] Every `\input{figures/...}` file exists in `book/figures/`.
- [ ] Every `\ref{fig:...}` has a matching `\label{fig:...}` in the TikZ file.
- [ ] Every `[@citation-key]` exists in `book/references.bib`.
- [ ] No em-dashes (`---` or `—`) anywhere in the text.
- [ ] No emojis.
- [ ] Opening connects to the previous chapter.
- [ ] Summary tees up the next chapter.
- [ ] Argument flows principle-first, then Smallworld's realization.
- [ ] Smallworld is not reintroduced; the reader already knows what it is.
- [ ] Stack choices (Rust, `wgpu`) are framed as Smallworld's, not universal prescriptions.
- [ ] Book compiles cleanly with `make pdf` from the `/book` folder
