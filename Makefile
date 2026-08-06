# smallworld task runner.
#
# A thin front end over cargo — no build logic lives here. `make ci` runs the same
# sequence as .github/workflows/ci.yml; keep the two in step when either changes.
# CI deliberately calls cargo directly rather than make, because GNU make is not
# guaranteed on the windows-latest runner image.
#
# Written for the stock macOS make (GNU make 3.81): no make-4-only syntax.

CARGO ?= cargo
SANDBOX := smallworld-sandbox

# RELEASE=1 make build|run
ifdef RELEASE
PROFILE_FLAG := --release
endif

.DEFAULT_GOAL := help
.PHONY: help fmt lint test build sandbox ci clean

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

sandbox: ## Run the sandbox
	$(CARGO) run -p $(SANDBOX) $(PROFILE_FLAG)

smoke: ## Run headless smoke test (adapter probe)
	$(CARGO) run -p $(SANDBOX) $(PROFILE_FLAG) -- --info

screenshot: ## Run viewer, capture window screenshot (DEST=path)
	@DEST=$${DEST:-/tmp/smallworld-screenshot.png}; \
	$(CARGO) run -p $(SANDBOX) $(PROFILE_FLAG) & \
	PID=$$!; \
	sleep $${DELAY:-6}; \
	WID=$$(osascript -e "tell application \"System Events\" to get id of first window of (first process whose unix id is $$PID)" 2>/dev/null); \
	if [ -n "$$WID" ]; then \
		screencapture -x -l "$$WID" "$$DEST"; \
		echo "captured $$DEST"; \
	else \
		echo "window not found"; \
	fi; \
	kill $$PID 2>/dev/null; wait $$PID 2>/dev/null

ci: lint test build smoke ## Everything ci.yml runs, in the same order

clean: ## Remove build artifacts
	$(CARGO) clean
