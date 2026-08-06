# Spec: 1 km world at target frame rate — SVO traversal optimization

## What

Bring the 1 km × 1 km world (10 cm terrain voxels, instanced objects) to the story's
acceptance criterion of **≥ 30 fps (≤ 33 ms GPU) with shadows on, 1280×720, M1 Max**.
Session baseline (2026-08-06, same-day A/B): 49.0 ms base / 83.9 ms shadows.

## Why

Epic proof-of-concept gate. Traversal is memory-latency bound — dependent random reads
into the 128 MB SVO node buffer. Optimization must cut dependent reads per ray, then
pack the remaining rays warp-dense.

## Phases as landed (revised twice from measurement; experiment matrix in issue comments)

### Phase 1 — single-read traversal (byte-identical)

Stack frames cache `children` + `child_mask`; each SVO node fetched exactly once per
ray (was ~k+2×). 49.0 → 39.9 ms base.

### Phase 2 — 8-byte stack frames via integer cell coords (3 px ulp diff)

Cell coords shift per descend/pop; `node_min` recomputed at classify (exact — parent
and child face planes coincide in f32). Frame 20 → 8 B, stack 320 → 128 B/thread.
39.9 → 32.4 ms base.

### Phase 3a — exact terrain-slab clip for shadow rays (byte-identical)

`terrain_top_y` plumbed through the dead `grid_dims` uniform slot;
`Raymarcher::set_terrain_top_y` called by world builders. Shadow rays prune traversal
past their slab exit. −2.8 ms shadows.

### Phase 3b — three traversal micro-optimizations

- **Occupancy masks**: 64-bit per brick (one bit per 4³ chunk), computed in
  `BrickPool::write_voxels`, gate the fine-DDA voxel fetch. Byte-identical, ~0 ms
  (brick DDA is iteration-bound, not read-bound) — kept: near-free, enables future use.
- **Bit-iteration child advance**: `firstTrailingBit` over a flip-permuted mask
  (3-op butterfly) replaces the 8-slot scan loop. Byte-identical, −3..−5 ms base.
- **Absolute AABB eps** (`VOXEL_SCALE * 0.02` replaces `node_size * 0.001`): the
  relative pad cost ~15% of frame in double-descents. ~−4 ms base; 494 px (large) /
  1828 px (default) isolated 1-px shading flips at grazing edges, crack-analysis clean.

### Phase 3c — half-res shadow pass (fallback b; quality user-approved)

Kernel split into three passes: `cs_primary` (full res, writes G-buffer: pos+ndotl
rgba32f, albedo rgba8, normal rgba8snorm), `cs_shadow` (half res, one ray per 2×2 quad,
warp-dense), `cs_shade` (composite; shadow pass skipped when shadows off).
Shadow marginal cost ~21 ms → ~3.5 ms. Image delta 0.47% (shadow-edge quantization) —
**user signed off 2026-08-06** from magnified side-by-side crops.

Two attempts discarded on measurement, documented in comments: in-pass 2×2 quad sharing
(saves work, not wall time under SIMD lockstep) and chunk-granular shadow any-hit
(crust chunks self-shadow every surface: 47.8% px).

### Out of scope (60 fps follow-up under epic sw-ff3ae3)

Beam pre-pass, node-buffer compaction/re-layout, wide trees, split-overhead
cool-machine A/B + G-buffer packing, CPU/streaming hitch reduction.

## Measurement protocol

- `BENCH_WORLD=large BENCH_SSE=0.8 cargo test --release -p smallworld-sandbox bench_raymarch -- --ignored --nocapture`
- Probe compare via `render_default_headless` (`BENCH_WORLD=large`, `PROBE_OUT`);
  absolute medians drift ±15% with machine temperature — deltas are same-run/interleaved.
- Shader-override A/B (`SMALLWORLD_SHADER_DIR`) for experiments, no rebuilds.
- Final: rested-machine bench + flythrough `--bench largeworld` p99 + full `cargo test`.

## Acceptance criteria

- [x] Phases 1–2: byte-identical/quantified probe, deltas reported (−34% base)
- [x] ≥ 30 fps with shadows on — **met on the flythrough** (GPU avg 20.9 / p99 23.4 ms
      ≤ 33 ms; user decision 2026-08-06 to validate on the flythrough as the
      representative experience). Fixed bench viewpoint reads ~35 ms (worst-case
      outlier, thermally unresolved) — tracked in the 60 fps follow-up.
- [x] Flythrough p99 reported: GPU p99 23.4 ms ✓ (no GPU hitches > 66 ms; dt p99
      47.5 ms is CPU/streaming-side, excluded per AC)
- [x] No regressions in `cargo test` (engine 30/30 release + 31/31 debug, sandbox green;
      pre-existing release-profile test bug filed+fixed as sw-031260)

## Decisions (user-confirmed 2026-08-06)

1. Shadow strategy: SSE multiplier → falsified; slab clip + occupancy masks → partial;
   half-res shadow **pass** (fallback b, pre-authorized) closes the gap.
2. Perf bar: 30 fps floor closes the story; 60 fps push is a follow-up.
3. Validation resolution: 1280×720.
4. AC closure: flythrough GPU p99 is the validation metric (user choice over holding
   the story open for a cool-machine bench-viewpoint A/B).
5. Half-res shadow quality: approved.