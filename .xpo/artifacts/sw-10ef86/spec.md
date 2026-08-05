# Spec — sw-10ef86: Scaffold Rust workspace: engine + viewer crates, CI for macOS/Windows/Linux

**Epic:** sw-4df655 (M0 Foundation & Toolchain) · **Points:** 2 · **Status:** review round 2, 2026-08-05

## What

Create the empty-but-real skeleton every later M0 story builds on:

1. A Cargo workspace with two crates — `engine` (library, owns rendering/world code) and
   `viewer` (binary, owns window/app shell).
2. Shared workspace configuration: resolver, MSRV, lint policy, dependency-version table.
3. **Shader directory layout** (WGSL) and the Rust-side accessor that loads them.
4. **Asset path conventions** — one resolution rule that works for `cargo run` *and* a
   shipped binary, so no later story invents its own.
5. GitHub Actions CI: fmt + clippy + build + test on macOS, Windows, Linux.
6. A `Makefile` fronting the same commands, so "what CI runs" is one `make ci` away
   (added review round 2).

## Why

DESIGN.md §1 fixes Rust + wgpu (D2) and WGSL shaders. Everything in M0 after this story
(`sw-a2446a` window/wgpu init, `sw-ce3a9f` raymarcher, `sw-c96adf` GPU timers) assumes the
workspace exists and that shaders and assets are found the same way on all three platforms.
The three-OS matrix exists specifically to catch the D2 portability risk (§13: "validate on
Metal early") on the *first* commit rather than the tenth — a CI matrix added later only
tells you something broke, not which commit broke it.

## Non-goals

- No `winit`, `wgpu`, or `egui` dependency, and no window — that is `sw-a2446a`.
- No actual shader *content* beyond one trivial placeholder proving the load path — the
  raymarch kernel is `sw-ce3a9f`.
- No release/packaging pipeline, no crates.io publishing, no cross-compilation.
- No `rust-toolchain.toml` (see D4).
- The `Makefile` is a convenience front-end only: no build logic lives in it that cargo
  does not already own (see D9).

## Layout

```
smallworld/
├── Cargo.toml                  # [workspace] — members, lints, dependency table, MSRV
├── Makefile                    # lint / test / build / run / ci / clean
├── .github/workflows/ci.yml
├── assets/                     # runtime-loaded data (textures, palettes, worlds)
│   └── .gitkeep
├── crates/
│   ├── engine/
│   │   ├── Cargo.toml
│   │   ├── shaders/            # WGSL — compiled into the binary
│   │   │   └── common.wgsl     # placeholder: shared constants/helpers
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── assets.rs       # asset root resolution
│   │       └── shaders.rs      # WGSL accessors
│   └── viewer/
│       ├── Cargo.toml
│       └── src/main.rs
└── docs/                       # existing
```

`crates/` prefix (rather than flat `engine/` + `viewer/`) keeps the root readable as the
crate count grows — DESIGN.md already foreshadows a game layer and possibly a separate
`world`/`physics` crate.

## Conventions established here

### Shaders

- Live in `crates/engine/shaders/`, one file per pass, `snake_case.wgsl`.
- Baked into the binary with `include_str!` — a shipped `viewer` can never fail to find a
  shader, and shader files are engine source, not user-facing assets.
- Accessed through `engine::shaders::load(Shader::X) -> Cow<'static, str>`, **not**
  `include_str!` at each call site. The indirection buys a dev-only override: if
  `SMALLWORLD_SHADER_DIR` is set, `load` reads the file from disk instead, so
  `sw-ce3a9f` can iterate on the raymarch kernel without recompiling. Baked constant is
  the default and the only path in a release build with the env var unset.
- WGSL has no `#include`. Shared code is composed in Rust:
  `engine::shaders::compose(&[Shader::Common, Shader::Raymarch])` concatenates sources.
  Defined now, exercised in `sw-ce3a9f`.

### Assets

- `assets/` at the workspace root; runtime data only (never shaders, never code).
- `engine::assets::root() -> PathBuf`, resolved once (`OnceLock`) in this order:
  1. `$SMALLWORLD_ASSETS` if set — explicit override for tests and packaging.
  2. `<dir of current exe>/assets` if it exists — shipped layout.
  3. `<workspace root>/assets` derived from `CARGO_MANIFEST_DIR` — dev layout, so
     `cargo run` works from any cwd.
- `engine::assets::path("textures/foo.png")` joins against the root. Paths are always
  written `/`-separated and joined component-wise, so Windows works without special cases.

### Lints

`[workspace.lints]` in the root manifest, inherited by both crates with
`[lints] workspace = true`: deny `unsafe_code` at the workspace level (individual modules
that need it — GPU buffer casts later — opt out explicitly and locally), warn on
`clippy::pedantic`-adjacent rules kept small for now (`clippy::all` + a handful), so CI's
`-D warnings` is meaningful rather than noisy.

### Dependency versions

`[workspace.dependencies]` is the single place versions are pinned; member crates use
`dep.workspace = true`. The table starts empty — entries are added by the story that first
needs the dependency (so `wgpu`/`winit`/`egui` land in `sw-a2446a`).

### Task runner (review round 2)

`make <target>` from the workspace root. Targets:

| Target | Runs | Notes |
|---|---|---|
| `help` | lists targets | default goal — bare `make` explains itself |
| `fmt` | `cargo fmt --all` | the only target that modifies files |
| `lint` | `cargo fmt --all --check` **+** `cargo clippy --workspace --all-targets -- -D warnings` | "everything CI rejects you for", without touching files |
| `test` | `cargo test --workspace` | |
| `build` | `cargo build --workspace` | `RELEASE=1 make build` adds `--release` |
| `run` | `cargo run -p smallworld-viewer` | honours `RELEASE=1` |
| `ci` | `lint` → `test` → `build` → `run` | the same sequence `ci.yml` runs |
| `clean` | `cargo clean` | |

`CARGO ?= cargo` so a different toolchain can be substituted. `make ci` and `ci.yml` are
kept in step by hand and each carries a comment pointing at the other (D9).

## Decisions

| # | Decision | Choice | Reasoning | Revisit if |
|---|----------|--------|-----------|------------|
| D1 | Package names | `smallworld-engine` / `smallworld-viewer`, in dirs `crates/engine` and `crates/viewer`; binary named `smallworld` | Directory names stay short (the issue's "engine"/"viewer"); package names stay unambiguous in `cargo` output and don't squat generic names if anything is ever published. The thing a user runs should be called `smallworld`, not `viewer`. **Confirmed by user, review round 1.** | We publish crates and want different names. |
| D2 | Edition | 2024 | Stable since 1.85; local toolchain is 1.94. No reason to start on an older edition. | — |
| D3 | Resolver | `resolver = "3"` | Edition 2024's default; makes MSRV-aware version selection explicit at the workspace level. | — |
| D4 | Toolchain pinning | **No** `rust-toolchain.toml`; set `rust-version = "1.94"` in the workspace | `rust-toolchain.toml` is a rustup feature and rustup is *not* installed on this machine (Homebrew rust) — the file would be silently ignored locally while pretending to guarantee something. CI pins `stable` explicitly instead. | The project gains rustup-based contributors or needs a nightly feature. |
| D5 | Linux CI system deps | Installed now, before `winit`/`wgpu` are dependencies | `sw-a2446a` adds them next; installing X11/Wayland/Vulkan dev packages now means the matrix stays green when it lands instead of going red on an unrelated-looking commit. Cost is ~20 s of apt on one runner. **Confirmed by user, review round 1.** | — |
| D6 | What `viewer` does | Prints engine version, resolved asset root, and shader-source byte count, then exits 0 | A binary that only prints `Hello` proves nothing. This exercises every convention the story introduces, and CI running it is a real smoke test. `sw-a2446a` replaces `main` with the window loop. | — |
| D7 | Test surface | Unit tests in `engine` for asset-root precedence and shader loading (incl. the env override) | These are the conventions later stories depend on; they are also the only logic in the story. Gives `cargo test` something real to run on all three OSes — notably path joining on Windows. | — |
| D8 | Shader delivery | Baked `include_str!` **plus** a `$SMALLWORLD_SHADER_DIR` dev override | Release builds stay self-contained; `sw-ce3a9f` can iterate on the raymarch kernel without a rebuild. **Confirmed by user, review round 1.** | The override becomes a footgun (stale shader dir silently used) — then gate it behind a `dev-shaders` feature. |
| D9 | CI does **not** call `make` | `ci.yml` invokes cargo directly; the `Makefile` mirrors it | GNU `make` is not guaranteed on the `windows-latest` runner image, and a task runner failing to *exist* is a confusing way to learn your build is fine. The duplication is four command lines, cross-referenced by comment in both files. **Requested by user, review round 2.** | We add a real (non-mirror) build step, or CI moves to a container where `make` is guaranteed — then CI should call `make ci` and the duplication goes away. |
| D10 | `lint` includes the format check | `make lint` = `fmt --check` + clippy | The question a developer is asking is "will CI reject this?", and formatting is one of the two ways it can. `make fmt` remains the only target that rewrites files. **Review round 2.** | Format churn makes the combined target annoying in practice. |

## Flow

1. Root `Cargo.toml`: `[workspace]` members, `resolver`, `[workspace.package]` (version,
   edition, rust-version, license), `[workspace.lints]`, empty `[workspace.dependencies]`.
2. `crates/engine`: `lib.rs` (re-exports `assets`, `shaders`, `VERSION`), `assets.rs`,
   `shaders.rs`, `shaders/common.wgsl` placeholder.
3. `crates/viewer`: `main.rs` per D6.
4. `assets/.gitkeep`.
5. `.gitignore`: add `/target`, `**/*.rs.bk`.
6. `.github/workflows/ci.yml`.
7. `Makefile` per the task-runner table.
8. Verify locally: `make ci` (equivalently `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace`,
   `cargo test --workspace`, `cargo run -p smallworld-viewer`).

## CI shape

```
name: CI
on: { push: { branches: [main] }, pull_request: }
concurrency: cancel superseded runs per ref
jobs:
  fmt:    ubuntu-latest — cargo fmt --all --check
  check:  matrix [ubuntu-latest, macos-latest, windows-latest]
          dtolnay/rust-toolchain@stable (clippy, rustfmt) + Swatinem/rust-cache
          ubuntu only: apt install libx11-dev libxkbcommon-dev libwayland-dev
                       libxrandr-dev libxi-dev libxcursor-dev mesa-vulkan-drivers
          cargo clippy --workspace --all-targets -- -D warnings
          cargo test  --workspace
          cargo run   -p smallworld-viewer      # D6 smoke test
```

`macos-latest` is Apple Silicon → Metal; `ubuntu-latest`/`windows-latest` cover the Vulkan
side of D2. No `RUSTFLAGS: -D warnings` at the env level (it churns the build cache);
`-D warnings` is passed to clippy only.

## Acceptance criteria

- [ ] `cargo build --workspace` succeeds on macOS.
- [ ] `cargo test --workspace` passes; engine tests cover asset-root precedence (all three
      branches) and shader load + env override.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- [ ] `cargo fmt --all --check` is clean.
- [ ] `cargo run -p smallworld-viewer` prints version, asset root, and shader byte count,
      and exits 0 — from a cwd other than the workspace root as well.
- [ ] `.github/workflows/ci.yml` defines fmt + a three-OS check matrix as above.
- [ ] `/target` is gitignored; `git status` is clean after a full build.
- [ ] `crates/engine/shaders/` exists with the `common.wgsl` placeholder and is reachable
      through `engine::shaders::load`.
- [ ] `make help`, `make lint`, `make test`, `make build`, `make run` and `make ci` all
      work from the workspace root on the stock macOS `make` (GNU make 3.81 — no
      make-4-only syntax), and `make ci` runs the same commands as `ci.yml`.

## Risks / notes

- **CI is unverifiable locally** — the repo has no git remote, so `ci.yml` cannot be
  observed green as part of this story. It is written to be conventional (`dtolnay`,
  `Swatinem`, standard runner labels) and is validated by review + YAML sanity only. First
  real run happens whenever a remote is added. Called out rather than hidden. `make ci`
  narrows this gap: the command sequence itself is verified locally even though the runner
  environment is not.
- **`Makefile`/`ci.yml` drift** — the accepted cost of D9. Mitigated by cross-referencing
  comments, and bounded because the Makefile holds no logic of its own.
- **Homebrew toolchain, no rustup** — `cargo +stable`-style invocations and
  `rust-toolchain.toml` do not work on this machine (D4).
- **`unsafe_code` deny** may need a local `#[allow]` as soon as GPU buffer casting appears;
  that is the intent (explicit, local opt-out), not an obstacle.

## Review round 1 (2026-08-05)

All three open questions resolved by the user; the recommended option was taken in each
case, so the spec above is unchanged in substance — see D1, D8, D5 for the confirmations.

1. **Package naming** → `smallworld-engine` / `smallworld-viewer`, binary `smallworld` (D1).
2. **Shader delivery** → baked `include_str!` + `$SMALLWORLD_SHADER_DIR` dev override (D8).
3. **CI forward-loading** → install Linux windowing/Vulkan dev packages now (D5).

## Review round 2 (2026-08-05)

User asked for a `Makefile` with `lint`, `test` and `build` targets. Added to scope as
item 6, with the task-runner table above and decisions D9 (CI keeps calling cargo directly)
and D10 (`lint` covers formatting too). `run`, `ci`, `clean` and a self-documenting `help`
round out the set at no extra concept cost; `ci` is the one that earns its keep, since it
is the only local approximation of a workflow this repo cannot yet execute.
