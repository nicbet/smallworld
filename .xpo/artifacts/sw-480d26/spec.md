# Benchmark harness

## What

A `--bench [preset]` CLI mode that opens the sandbox with full rendering, runs a scripted camera orbit for a fixed duration, collects per-frame metrics, and prints a summary report on exit.

## Why

Quantitative performance data before/after engine changes — comparable across runs because the camera path is deterministic. More useful than a headless smoke test: it exercises the real rendering pipeline and produces actionable numbers.

## Design

### CLI

```
smallworld --bench                  # bench Default preset, 20s
smallworld --bench TerrainOnly      # bench a specific preset
smallworld --bench --duration 10    # override duration
```

### Camera path

Deterministic orbit around the scene center:
- Radius: enough to see the full scene (derived from grid bounds or preset)
- Height: slightly above the scene
- Speed: one full orbit over the bench duration
- No user input — mouse/keyboard ignored during bench

### Metrics collected per frame

| Metric | Source |
|---|---|
| `dt_ms` | wall-clock delta between frames |
| `cpu_ms` | `Instant` around the frame's CPU work |
| `gpu_compute_ms` | GPU timestamp: compute pass |
| `gpu_blit_ms` | GPU timestamp: blit pass |
| `gpu_egui_ms` | GPU timestamp: egui pass |
| `bricks` | `brick_pool.live_count()` |
| `instances` | `scene.instance_count()` |

### Report

Printed to stdout on exit:

```
smallworld bench — Default, 20.0s, 1187 frames
──────────────────────────────────────────────────
              min      avg      max      p99
  dt       0.42    16.85    33.21    31.50  ms
  cpu      0.08     0.32     1.24     0.98  ms
  gpu     11.20    14.62    22.40    20.10  ms
  fps      30.1     59.3     60.0     31.7

  bricks: 4812    instances: 148
──────────────────────────────────────────────────
```

### Makefile

```
bench: ## Run 20s benchmark on Default preset
    $(CARGO) run -p $(SANDBOX) $(PROFILE_FLAG) -- --bench
```

Keep existing `smoke` target (`--info`) unchanged for CI.

## Flow

1. Parse `--bench` args in `main()` — extract optional preset name and `--duration`
2. Add `BenchState` struct: duration, elapsed, frame metrics vec, orbit params
3. In `resumed()`, if bench mode: set up orbit camera instead of free camera, store `BenchState`
4. In `RedrawRequested`, if bench mode:
   - Advance orbit camera by dt
   - Push frame sample to metrics vec
   - After duration elapsed, print report and `event_loop.exit()`
5. Disable input handling during bench (no WASD/mouse)
6. Egui overlay still renders (its cost is part of the benchmark)

## Acceptance Criteria

- [ ] `cargo run -p smallworld-sandbox -- --bench` runs Default for 20s, prints report, exits 0
- [ ] `--bench TerrainOnly` runs the correct preset
- [ ] `--bench --duration 5` runs for 5 seconds
- [ ] Camera orbits deterministically (same run = same path)
- [ ] Report shows min/avg/max/p99 for dt, cpu, gpu, fps
- [ ] Report shows brick and instance counts
- [ ] `make bench` target works
- [ ] All existing checks pass (fmt, clippy, tests)
