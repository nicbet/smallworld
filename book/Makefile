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
#   make clean    — remove build artifacts
#   make check    — verify quarto is installed and list chapters

QUARTO  ?= quarto
OUTDIR  := _output
PDF     := $(OUTDIR)/Modern-Game-Engine-Design.pdf

# All markdown chapter sources (auto-discovered, sorted)
CHAPTERS := $(sort $(wildcard [0-9][0-9]-*.md))
SOURCES  := index.qmd _quarto.yml $(CHAPTERS)

.PHONY: all pdf preview clean check

all: pdf

# ── Build ──────────────────────────────────────────────────────────────
pdf: check $(SOURCES)
	$(QUARTO) render --to pdf

# ── Preview (live-reload) ──────────────────────────────────────────────
preview: check
	$(QUARTO) preview

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
