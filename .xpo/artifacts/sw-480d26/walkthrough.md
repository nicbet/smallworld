# Walkthrough: Benchmark harness

## What changed

Added a `--bench` CLI mode that opens the sandbox, runs a scripted camera orbit for a configurable duration, collects per-frame metrics, and prints a summary report on exit.

## New file: `crates/sandbox/src/bench.rs`

### Types

- `BenchConfig` — preset + duration, parsed from CLI args
- `BenchState` — runtime state: start time, sample buffer, orbit parameters
- `BenchSample` — per-frame metrics: dt, cpu, gpu (compute/blit/egui split)

### Orbit camera

`advance_orbit(dt, camera)` computes a circular orbit:
- One full revolution over the bench duration (`TAU / duration_secs` radians per second)
- Radius and height derived from the preset's grid bounds (60% of horizontal extent, 60% of vertical extent above center)
- Camera yaw/pitch pointed at the orbit center using `atan2`
- `Vec3Swizzles` trait imported for `.xz()` on the look-at vector

### Report

`print_report()` collects sorted arrays for dt, cpu, gpu, and fps, computing min/avg/max/p99 for each. p99 is the 99th percentile index into the sorted array. Output is a fixed-width table to stdout.

### Arg parsing

`parse_args()` scans for `--bench`, then optional preset name (case-insensitive, with or without spaces: "TerrainOnly" or "Terrain Only") and `--duration N`. Returns `None` if `--bench` isn't present.

## Modified: `crates/sandbox/src/main.rs`

### App initialization

- `App` gains `bench_config: Option<BenchConfig>`, consumed in `resumed()` to build `BenchState`
- `RunState` gains `bench: Option<BenchState>`
- When bench mode is active, the preset comes from the bench config rather than defaulting to `Preset::Default`

### Frame loop changes

- Camera movement: bench mode calls `bs.advance_orbit(dt)` instead of reading WASD input
- After recording the frame sample, bench mode pushes a `BenchSample` and checks `is_done()`
- On completion: prints report with brick/instance counts and calls `event_loop.exit()`
- `device_event` (mouse look) returns early during bench

### Egui

The debug overlay still renders during bench — its GPU cost is part of the measurement, matching real-world rendering conditions.

## Modified: `Makefile`

New `bench` target with `PRESET` and `DURATION` environment variable overrides:
```
make bench                    # Default, 20s
PRESET=Stress make bench      # Stress preset
DURATION=10 make bench        # 10 second run
```

## Key decisions

- **Orbit parameters from grid bounds, not hardcoded** — each preset gets a sensible orbit automatically. Empty/SingleBrick scenes get the 5m/3m minimums.
- **GPU timestamps are EMA-smoothed** — same source as the debug panel. First few frames show 0.0 while the EMA warms up; this is visible in the min column.
- **No warmup skip** — all frames count, including the first few with cold caches and EMA ramp-up. The p99 metric is the most useful for performance decisions; outlier warmup frames affect min/max but not p99.
