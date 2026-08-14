# Quarto Format Conventions

## Chapter structure

```
# Chapter Title

Opening paragraphs connecting to previous chapter.

## What You Will Learn in This Chapter {.unnumbered}

- **Bold Label:** Description of concept.
- **Bold Label:** Description of concept.

## First Major Section

Prose...

### Subsection

Prose...

## Chapter Summary {.unnumbered}

Numbered wrap-up of key points, tee up the next chapter.

## Review Questions {.unnumbered}

1. Question?
2. Question?
```

- **Chapter title:** `# Title` (H1), one per file.
- **Opening:** 1-3 paragraphs connecting to the previous chapter and framing this one.
- **Learning objectives:** `## What You Will Learn in This Chapter {.unnumbered}` with bold-labeled bullet points.
- **Sections:** `##` (numbered implicitly by Quarto, e.g. 5.1). `###` for subsections, `####` for sub-subsections.
- **Closing:** `## Chapter Summary {.unnumbered}` then `## Review Questions {.unnumbered}`.

## Code listings

Annotate with Quarto's cross-referenceable listing syntax:

````
@lst-my-listing shows the pattern:

```{#lst-my-listing .rust lst-cap="Caption describing the listing"}
struct Foo {
    bar: u32,
}
```
````

**Every listing MUST be cross-referenced** in the prose before it appears, using
`@lst-name`. Never leave a listing unreferenced.

**Listings float.** Like figures, code listings may not appear directly below
the sentence that references them. Write the reference so that the sentence
stands on its own: describe what the listing *contains* rather than saying
"as shown below." Good: "@lst-time-struct defines the `Time` struct, which
encodes three clocks and an accumulator." Bad: "The struct is shown in
@lst-time-struct:" (implies spatial adjacency that floating breaks).

## Bibliography

- Cite with `[@key]`.
- Add new entries to `book/references.bib` when needed (`@online{...}` for web, `@book{...}` for books).
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
