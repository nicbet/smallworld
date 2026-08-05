# smallworld task runner.
#
# A thin front end over cargo — no build logic lives here. `make ci` runs the same
# sequence as .github/workflows/ci.yml; keep the two in step when either changes.
# CI deliberately calls cargo directly rather than make, because GNU make is not
# guaranteed on the windows-latest runner image.
#
# Written for the stock macOS make (GNU make 3.81): no make-4-only syntax.

CARGO ?= cargo
VIEWER := smallworld-viewer

# RELEASE=1 make build|run
ifdef RELEASE
PROFILE_FLAG := --release
endif

.DEFAULT_GOAL := help
.PHONY: help fmt lint test build run ci clean

# Targets share one cargo target directory; serialise them even under `make -j`.
.NOTPARALLEL:

help: ## Show this help
	@echo "smallworld — make targets"
	@echo
	@grep -E '^[a-z][a-z-]*:.*## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN { FS = ":.*## " } { printf "  %-7s %s\n", $$1, $$2 }'
	@echo
	@echo "  RELEASE=1 make build|run   builds with --release"

fmt: ## Format the workspace in place (the only target that edits files)
	$(CARGO) fmt --all

lint: ## rustfmt --check + clippy with warnings denied
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test: ## Run the workspace test suite
	$(CARGO) test --workspace

build: ## Build every crate in the workspace
	$(CARGO) build --workspace $(PROFILE_FLAG)

run: ## Run the viewer
	$(CARGO) run -p $(VIEWER) $(PROFILE_FLAG)

ci: lint test build run ## Everything ci.yml runs, in the same order

clean: ## Remove build artifacts
	$(CARGO) clean
