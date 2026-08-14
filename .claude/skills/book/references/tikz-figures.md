# TikZ Figure Conventions

Figures are TikZ diagrams in `book/figures/`, rendered natively in the PDF.

## Referencing from chapter text

```
Figure \ref{fig:my-figure} shows the relationship.

\input{figures/my-figure.tex}
```

Always mention the figure in prose before the `\input{}`.

## File template

```latex
\begin{figure}[!tb]
\centering
\begin{tikzpicture}[>=Latex, x=1cm, y=1cm]
  % diagram content here
\end{tikzpicture}
\caption{Description of what the figure shows.}
\label{fig:my-figure}
\end{figure}
\FloatBarrier
```

## Available TikZ styles (from `book/preamble.tex`)

These are pre-defined in the book's preamble and available in all figures:

- `engine-layer` -- outer grouping box (draw=black!55, fill=black!4, rounded corners)
- `engine-package` -- inner content box (draw=black!50, fill=white, rounded corners, sffamily scriptsize)
- `engine-boundary` -- emphasized boundary box (draw=black!55, fill=black!12, bold scriptsize)

## Style rules

- Arrow style: `>=Latex`
- Fonts: `\sffamily\scriptsize` for labels, `\sffamily\bfseries\scriptsize` for headings, `\sffamily\tiny` for annotations
- Line widths: 0.35-0.6pt (0.4-0.5pt most common)
- Dashed lines for optional/advisory connections, solid for primary data flow
- Use `black!50`-`black!70` for muted elements, full black for emphasis
