---
name: chapter-writing
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
7. **Scan `book/figures/`** to see existing TikZ conventions.

## Core thesis

Decades of game engine lessons, keep the proven winners, discard the rest, apply
Rust and wgpu for a clean 2026 starting point. `docs/architecture.md` is the spec;
the book is the architectural textbook around it. Explain the _why_ behind each
decision, compare industry approaches, and use Smallworld as the concrete example.

## Quarto format conventions

- **Chapter title:** `# Title` (H1), one per file.
- **Opening:** 1-3 paragraphs connecting to the previous chapter and framing this one.
- **Learning objectives:** `## What You Will Learn in This Chapter {.unnumbered}` with bold-labeled bullet points.
- **Sections:** `##` (numbered implicitly by Quarto, e.g. 5.1). `###` for subsections, `####` for sub-subsections.
- **Closing:** `## Chapter Summary {.unnumbered}` then `## Review Questions {.unnumbered}`.

## Prose style

- First-person plural ("we") and direct address.
- Meaty, verbose paragraphs. Each section should have substantial discussion.
- Balanced: explain what approaches exist, why the chosen one wins, common pitfalls.
- Compare with UE5, Unity, Godot where relevant.
- Include historic relevance and background ("Unity solved this that way because they had to support...")
- Show how Rust and wgpu enable cleaner solutions.
- Back up claims with citations from web articles, scientific research (papers, journal articles, theses)
- **NO emojis.**
- **NO em-dashes** (no `---`, no `—`). Link clauses naturally with colons, commas, semicolons, or restructure the sentence.

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
- [ ] Book compiles cleanly with `make pdf` from the `/book` folder
