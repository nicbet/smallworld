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
#   make watch    — rebuild the PDF when any Markdown file changes
#   make clean    — remove build artifacts
#   make check    — verify quarto is installed and list chapters

QUARTO  ?= quarto
FSWATCH  ?= fswatch
OUTDIR  := _output
PDF     := $(OUTDIR)/Modern-Game-Engine-Design.pdf

# All markdown chapter sources (auto-discovered, sorted)
CHAPTERS := $(sort $(wildcard [0-9][0-9]-*.md))
SOURCES  := index.qmd _quarto.yml $(CHAPTERS)

.PHONY: all pdf preview watch clean check check-watch

all: pdf

# ── Build ──────────────────────────────────────────────────────────────
pdf: check $(SOURCES)
	$(QUARTO) render --to pdf

# ── Preview (live-reload) ──────────────────────────────────────────────
preview: check
	$(QUARTO) preview

# ── Watch (rebuild PDF on Markdown changes) ────────────────────────────
watch: check check-watch pdf
	@echo "Watching Markdown files under $(CURDIR) (Ctrl-C to stop)..."
	@$(FSWATCH) -o -r --include '.*\.md$$' --exclude '.*' . | \
		while read -r _; do \
			$(MAKE) --no-print-directory pdf; \
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
