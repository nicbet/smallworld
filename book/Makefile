# Book Build — Quarto → PDF
#
# Prerequisites:
#   brew install quarto          (or https://quarto.org/docs/get-started/)
#   Quarto's PDF backend needs a TeX distribution:
#     quarto install tinytex     (lightweight, recommended)
#
# Usage:
#   make          — build the PDF (default)
#   make pdf      — same as above
#   make preview  — live-preview in the browser (auto-rebuilds on save)
#   make watch    — rebuild the PDF with Quarto's execution cache on source changes
#   make clean    — remove build artifacts
#   make check    — verify quarto is installed and list chapters

QUARTO  ?= quarto
FSWATCH  ?= fswatch
RENDER_FLAGS ?=
OUTDIR  := _output
PDF     := $(OUTDIR)/The-Modern-Game-Engine.pdf

# Chapter sources and the files that affect PDF output.
CHAPTERS := $(sort $(wildcard chapters/*.qmd))
SOURCES  := index.qmd introduction.qmd _quarto.yml preamble.tex dropcap.lua support/title-page.pdf $(CHAPTERS)

# Graphviz diagrams: .dot → .pdf (vector output, no rasterisation).
DOT_SRCS := $(wildcard figures/*.dot)
DOT_PDFS := $(DOT_SRCS:.dot=.pdf)

.PHONY: all pdf preview watch clean check check-watch figures

all: pdf

# ── Figures ───────────────────────────────────────────────────────────
figures: $(DOT_PDFS)

figures/%.pdf: figures/%.dot
	dot -Tpdf -o $@ $<

# ── Build ──────────────────────────────────────────────────────────────
pdf: check figures $(SOURCES)
	$(QUARTO) render --to pdf $(RENDER_FLAGS)

# ── Preview (live-reload) ──────────────────────────────────────────────
preview: check
	$(QUARTO) preview

# ── Watch (rebuild cached PDF on book-source changes) ──────────────────
# Quarto's cache reuses executable-cell results (including generated
# diagrams) when their inputs are unchanged. It does not skip PDF typesetting.
watch: check check-watch
	@$(MAKE) --no-print-directory pdf RENDER_FLAGS="--cache"
	@echo "Watching book sources under $(CURDIR) with Quarto cache (Ctrl-C to stop)..."
	@$(FSWATCH) -o -r --include '.*\.(md|qmd|yml|yaml|tex|lua|mmd|dot)$$' --exclude '.*' . | \
		while read -r _; do \
			$(MAKE) --no-print-directory pdf RENDER_FLAGS="--cache"; \
		done

# ── Clean ──────────────────────────────────────────────────────────────
clean:
	rm -rf $(OUTDIR)
	rm -rf .quarto

# ── Preflight ──────────────────────────────────────────────────────────
check:
	@command -v $(QUARTO) >/dev/null 2>&1 || { \
		echo "Error: quarto not found. Install via:"; \
		echo "  brew install quarto"; \
		echo "  # or: https://quarto.org/docs/get-started/"; \
		exit 1; \
	}
	@echo "quarto $(shell $(QUARTO) --version)"
	@echo "Chapters: $(CHAPTERS)"

check-watch:
	@command -v $(FSWATCH) >/dev/null 2>&1 || { \
		echo "Error: fswatch not found. Install via:"; \
		echo "  brew install fswatch"; \
		exit 1; \
	}
