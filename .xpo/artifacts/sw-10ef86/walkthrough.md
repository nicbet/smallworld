# Walkthrough — Rust workspace scaffold, shader/asset conventions, CI

This is the first code in the repo. Nothing here renders anything; the point is that every
M0 story after it (`sw-a2446a` window + wgpu, `sw-ce3a9f` raymarcher, `sw-c96adf` GPU
timers) inherits a working build, a place to put shaders, a way to find assets, and a
three-platform CI that fails on the commit that broke it rather than ten commits later.

## The shape

```
Cargo.toml                       workspace: members, resolver 3, edition 2024,
                                 MSRV 1.94, shared lints, dependency-version table
Makefile                         help / fmt / lint / test / build / run / ci / clean
.github/workflows/ci.yml         fmt job + check matrix (ubuntu, macos, windows)
assets/                          runtime data root
crates/engine/                   package smallworld-engine, lib smallworld_engine
  shaders/common.wgsl            WGSL, baked into the binary
  src/{lib,assets,shaders}.rs
crates/viewer/                   package smallworld-viewer, binary `smallworld`
  src/main.rs
```

Package names are `smallworld-*` while the directories stay short; the binary is
`smallworld`, because the thing a user launches should be named after the project, not
after its role in the workspace. `crates/` as a container is for the crates DESIGN.md
already implies are coming (game layer, possibly separate world/physics).

## The two conventions this story exists to fix

Both are *functions*, not constants copied to call sites. That is the whole point: if the
next story reaches for `include_str!` or `PathBuf::from("assets/…")` directly, the
convention has already failed.

### Shaders — `crates/engine/src/shaders.rs`

Shaders are engine **source**, not user-facing assets, so they are baked in with
`include_str!` and a shipped binary can never fail to find one. But baking alone makes
shader iteration a rebuild, and `sw-ce3a9f` is going to spend a lot of time editing a
raymarch kernel. So the accessor does both:

```rust
pub fn load(shader: Shader) -> Cow<'static, str> {
    let overridden = env::var_os(SHADER_DIR_ENV).and_then(|dir| read_from(Path::new(&dir), shader));
    match overridden {
        Some(source) => Cow::Owned(source),   // $SMALLWORLD_SHADER_DIR — dev iteration
        None => Cow::Borrowed(shader.baked()), // include_str! — the shipped path
    }
}
```

`Cow` is what makes this free in the normal case: with the variable unset there is no
allocation and no filesystem call, just a `&'static str`. An *unreadable* override warns on
stderr and falls back rather than failing — a typo in an env var should not take the
renderer down mid-session.

WGSL has no `#include`, so shared declarations live in `Shader::Common` and get prepended
in Rust by `compose(&[Shader::Common, Shader::Raymarch])`. Each source is preceded by a
`// ---- common.wgsl ----` marker, which is what makes a naga error's line number traceable
back to a file once modules are concatenated.

`Shader` is `#[non_exhaustive]`: every M0 story adds a variant, and downstream `match`es
should not break when they do.

The test worth understanding is `file_names_match_the_baked_files`. It reads each shader
off disk through `read_from` and asserts it equals `baked()`. That looks tautological but
is not: `file_name()` (used by the override path) and the `include_str!` argument (used by
the baked path) are two independent strings. If they ever drift, the override would
silently serve a *different file* than the one compiled in — the kind of bug that costs an
afternoon. This test makes that impossible.

### Assets — `crates/engine/src/assets.rs`

```
$SMALLWORLD_ASSETS  →  <dir of running exe>/assets  →  <workspace root>/assets
```

Explicit override first (packaging, tests), then the shipped layout, then the dev layout
derived from the compile-time `CARGO_MANIFEST_DIR` — which is why `cargo run` works from
any working directory. Resolved once into a `OnceLock`.

The non-obvious part is why `resolve` takes its inputs as parameters instead of reading the
environment itself:

```rust
fn resolve(override_dir: Option<PathBuf>, exe_dir: Option<&Path>) -> PathBuf
```

Edition 2024 makes `env::set_var` **unsafe**, and the workspace denies `unsafe_code`. A test
that wanted to exercise the precedence rules by setting the variable simply could not
compile — and even with an `#[allow]` it would race the other tests, since env vars are
process-global and cargo runs tests in threads. Passing the inputs in makes all three
branches testable with no environment mutation at all. `shaders::read_from` takes a
directory for the same reason. **If you add a new environment-driven behaviour here, follow
this shape** rather than reaching for an `#[allow(unsafe_code)]`.

`path("textures/stone/albedo.png")` splits on `/` and joins component-wise, so relative
paths are written one way in source and are still correct on Windows. The Windows runner in
the matrix is what keeps that honest.

## Lints, and why `-D warnings` is meaningful

`[workspace.lints]` in the root manifest, inherited by both crates via `[lints] workspace =
true`:

- `unsafe_code = "deny"` — GPU buffer casting will eventually need unsafe; the intent is
  that such a module opts out *locally and visibly* with `#[allow(unsafe_code)]` and a
  comment, rather than the whole tree being open by default.
- `missing_docs = "warn"` — combined with CI's `-D warnings`, public engine API must be
  documented. Cheap to hold from commit one, expensive to retrofit.
- `clippy::all` + `clippy::dbg_macro`.

The set is deliberately small. `-D warnings` is only a useful gate if a warning means
something is actually wrong; a noisy lint set trains everyone to add `#[allow]`.

## CI and the Makefile

`ci.yml` runs two jobs: `fmt` on ubuntu, and a `check` matrix over ubuntu/macos/windows
doing clippy `-D warnings` → `cargo test` → `cargo run -p smallworld-viewer`. `macos-latest`
is Apple Silicon, so it is the Metal target; ubuntu and windows cover the Vulkan side of
DESIGN.md's D2 portability risk.

Two choices worth knowing:

- **The Linux X11/Wayland/Vulkan apt step is already there**, before anything depends on it.
  `sw-a2446a` adds winit and wgpu next; without this the matrix would go red on a commit
  whose diff contains no CI changes at all, which is a confusing way to learn about a
  missing system package.
- **`RUSTFLAGS: -D warnings` is deliberately not set at the env level.** It participates in
  cargo's fingerprint, so setting it globally invalidates the `Swatinem/rust-cache` entry
  between clippy and build. `-D warnings` is passed to clippy as an argument instead.

The `Makefile` mirrors that sequence as `make ci`, so the workflow can be rehearsed
locally — which matters more than usual here, because **this repo has no git remote yet and
`ci.yml` has therefore never actually executed**. It parses correctly and uses conventional
actions, but "CI is green" is unverified until a remote exists. Do not treat the badge-shaped
confidence as earned yet.

CI does *not* call `make`. GNU make is not guaranteed on the `windows-latest` runner image,
and a task runner failing to *exist* is a terrible way to learn your build is fine. The cost
is four duplicated command lines; both files carry a comment pointing at the other. If CI
ever moves to a container where make is guaranteed, collapse the duplication.

`make lint` includes `cargo fmt --all --check` as well as clippy, because the question a
developer is actually asking is "will CI reject this?" — and formatting is one of the two
ways it can. `make fmt` stays the only target that rewrites files. `.NOTPARALLEL:` is set
since every target contends for one cargo target directory, and the file avoids make-4-only
syntax so the stock macOS make (3.81) works.

## What `viewer` does, and why it does anything at all

```
smallworld-viewer 0.1.0
  engine      0.1.0
  host        macos/aarch64
  assets      /Volumes/Work/Projects/smallworld/assets
  shaders     common.wgsl (845 bytes)
```

A binary that prints `Hello` proves the toolchain links. This prints the output of every
convention the story introduces, which means CI running it is a real smoke test on all three
platforms — and the byte count is a live indicator of which shader source path was taken
(845 baked vs. whatever an override supplies). `sw-a2446a` replaces `main` with the window
loop; keep the reporting somewhere, ideally in the egui overlay.

## Verified

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo build --workspace` clean; `cargo test --workspace` passes 11 tests. The viewer was
exercised from the workspace root, from an unrelated working directory, under
`SMALLWORLD_ASSETS`, under a valid `SMALLWORLD_SHADER_DIR` (845 → 40 bytes), and under a
bogus one (warned, fell back, exit 0). `make ci` exits 0, and was confirmed to *fail*
(exit 2, stopping at `lint`) when a misformatted function was introduced — a gate that
never fails is decoration.

## Deliberately not here

winit, wgpu, egui, any real shader content, `rust-toolchain.toml` (rustup is not installed
on this machine, so the file would be silently ignored while implying a guarantee), and a
`license` field (no license has been chosen; `publish = false` instead).

`[workspace.dependencies]` currently holds only the internal `smallworld-engine` path dep.
The convention is that the story which first needs a third-party crate adds its version
there, and member crates write `dep.workspace = true` — so versions are unified in exactly
one place from the start.
