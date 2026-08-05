## What was built

GPU timestamp queries around every render pass, with two visualization layers: numeric per-pass timings in the debug panel, and a scrolling frame time graph that shows CPU, GPU, and wall-clock history at a glance.

## How the pieces fit together

### GPU timing infrastructure (`crates/engine/src/gpu_timing.rs`)

`GpuTimestamps` owns three GPU resources:
- A **query set** (`QueryType::Timestamp`, 2 queries per pass — begin and end)
- A **resolve buffer** (`QUERY_RESOLVE | COPY_SRC`) that receives raw tick values
- A **readback buffer** (`MAP_READ | COPY_DST`) that the CPU can read

Each frame follows this sequence:
1. `read_results()` at frame start — maps the readback buffer (written by the previous frame), converts tick deltas to milliseconds using `queue.get_timestamp_period()`, updates EMA averages (alpha=0.05), then unmaps
2. All three passes record their timestamp writes (indices 0/1 for compute, 2/3 for blit, 4/5 for egui)
3. `resolve()` after all passes — `resolve_query_set` writes ticks to the resolve buffer, then `copy_buffer_to_buffer` copies to the readback buffer

The readback is synchronous: `map_async` + `device.poll(wait_indefinitely())`. Under VSync the previous frame's GPU work is always complete, so the poll returns immediately. This avoids async double-buffering complexity.

### Feature negotiation (`crates/engine/src/gpu.rs`)

`negotiate_features()` checks if the adapter supports `TIMESTAMP_QUERY` and requests it if available. Both the windowed and headless paths use this. `supports_timestamps()` lets the viewer decide whether to create `GpuTimestamps`.

### Raymarcher changes (`crates/engine/src/raymarcher.rs`)

`render()` gained two optional parameters: `compute_timestamps: Option<ComputePassTimestampWrites>` and `blit_timestamps: Option<RenderPassTimestampWrites>`. These are passed directly to the compute and blit pass descriptors. When `None`, the passes record no timestamps — identical behavior to before.

### Viewer integration (`crates/viewer/src/main.rs`)

**RunState** gained `timestamps: Option<GpuTimestamps>` and `frame_history: FrameHistory`.

**FrameHistory** is a fixed-size ring buffer of 300 `FrameSample` entries. Each sample records:
- `dt_ms` — wall-clock time between frames (includes VSync blocking)
- `cpu_ms` — `Instant` delta from frame start to after submit+present
- `gpu_ms` — sum of GPU pass EMA averages from `GpuTimestamps`

A new sample is pushed at the end of every `RedrawRequested`.

**Debug panel** shows per-pass GPU timings as text (Compute / Blit / egui / Total), or "GPU: N/A" when timestamps are unsupported.

**Frame Time graph** is a separate collapsible egui window that renders the ring buffer as a scrolling vertical bar chart:
- Newest frames on the right, oldest scroll off the left
- Three layers per frame column: GPU bar (green), CPU bar (blue, translucent), dt tick mark (yellow; red if over 33.33 ms)
- Auto-scaling Y axis, minimum 16.67 ms so the 60 FPS line is always visible
- Reference lines at 16.67 ms (60 FPS) and 33.33 ms (30 FPS) with labels
- Hover tooltip shows a color legend
- All rendering via `egui::Painter` — no external dependencies

### Why three layers instead of stacked bars

CPU and GPU work are pipelined — the CPU encodes frame N+1 while the GPU executes frame N. Stacking them would overstate total time. Overlaid bars with different opacity show each measurement independently, and the dt tick mark shows the actual frame cadence.

## Key decisions

- **Synchronous readback** rather than async double-buffering. `poll(wait_indefinitely())` at frame start blocks until the previous frame's GPU work is done. Under VSync this is effectively free. The simplicity is worth it for a debug tool.

- **EMA for text, raw samples for graph.** The debug panel uses EMA-smoothed averages (readable, stable numbers). The frame time graph stores raw per-frame samples so spikes and trends are visible.

- **Custom egui widget rather than puffin_egui.** puffin_egui 0.30 depends on egui 0.33, which is a type mismatch with our egui 0.36. The custom implementation is ~100 lines of `Painter` calls with zero dependencies.

- **wgpu 30 API specifics.** `PollType::wait_indefinitely()` replaces the older `Maintain::Wait`. `get_mapped_range()` now returns `Result`. `timestamp_writes` on pass descriptors takes `Option<T>` not `Option<&T>`.

## What a future reader should know

- Pass indices are assigned by the viewer (0=compute, 1=blit, 2=egui) and are just offsets into the query set. Adding a new pass means bumping the pass count in `GpuTimestamps::new` and assigning the next index.

- The `has_data` flag in `GpuTimestamps` skips readback on the first frame when the readback buffer hasn't been written to yet.

- `wrapping_sub` on timestamp ticks handles GPU clock wrap-around, though with 64-bit counters this is unlikely in practice.

- The ring buffer's `iter_newest_first()` yields samples from most recent to oldest, which maps directly to the right-to-left bar rendering.
