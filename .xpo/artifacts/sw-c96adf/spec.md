## What

GPU timestamp queries around the compute, blit, and egui passes, with per-pass rolling averages in the egui debug overlay and a scrolling frame time graph showing CPU, GPU, and wall-clock frame time history. Gracefully degrades to "N/A" when the adapter doesn't support `TIMESTAMP_QUERY`.

## Why

Measurement discipline from day one. CPU frame time alone conflates CPU and GPU work. GPU timestamps isolate per-pass cost, and the frame time graph makes spikes and trends immediately visible.

## Acceptance Criteria

- Each GPU pass (compute, blit, egui) has begin/end timestamp queries
- Debug overlay shows per-pass GPU time in ms (EMA-smoothed) and a total
- Frame Time window shows a scrolling bar chart (300 frames) with CPU, GPU, and dt bars
- Reference lines at 60 FPS (16.67 ms) and 30 FPS (33.33 ms)
- When `TIMESTAMP_QUERY` is unsupported, debug panel shows "N/A", graph shows CPU + dt only
- `cargo clippy` and `cargo test` pass

## Flow

### 1. Feature negotiation — `crates/engine/src/gpu.rs`

`negotiate_features()` conditionally requests `TIMESTAMP_QUERY`. `supports_timestamps()` accessor.

### 2. New module — `crates/engine/src/gpu_timing.rs`

`GpuTimestamps` struct: query set, resolve buffer, MAP_READ readback buffer, EMA averages. Synchronous readback via `map_async` + `poll(wait_indefinitely())`.

### 3. Raymarcher — `crates/engine/src/raymarcher.rs`

`render()` accepts optional `ComputePassTimestampWrites` and `RenderPassTimestampWrites`.

### 4. Viewer — `crates/viewer/src/main.rs`

**Frame history ring buffer** — `FrameHistory` holds 300 `FrameSample` entries, each with `dt_ms` (wall-clock), `cpu_ms` (CPU work), `gpu_ms` (sum of GPU pass averages). New sample pushed at end of each frame.

**Debug panel** — numeric GPU Compute / Blit / egui / Total. "GPU: N/A" when unsupported.

**Frame Time graph** — collapsible egui window:
- Scrolling vertical bar chart, newest frames on the right
- Three layers per frame: GPU bar (green), CPU bar (blue, translucent), dt tick mark (yellow, red if > 33 ms)
- Auto-scaling Y axis, clamped to at least 16.67 ms
- Reference lines at 60 FPS and 30 FPS with labels
- Hover tooltip shows legend

## Decisions

**D1: Synchronous readback via `poll(wait_indefinitely)`.** Simple and correct for a debug tool.

**D2: Ring buffer of 300 samples, not EMA for the graph.** The graph needs per-frame history, not smoothed averages. EMA is kept for the debug panel text.

**D3: Three layers per bar (GPU, CPU, dt), not stacked.** GPU and CPU overlap in time (pipelined), so stacking would overstate total time. Overlaid bars with different opacity show each independently.

**D4: 2px bar width, no gaps.** At 300 frames × 2px = 600px, the graph fits in a reasonably sized window. Dense bars make trends and spikes obvious.
