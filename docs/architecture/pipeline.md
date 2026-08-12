## The Frame Pipeline Architecture

### The 2-Frame Pipeline

Smallworld uses a **2-frame pipeline** where the Game Thread and Render Thread overlap by one frame. This is a deliberate simplification of UE5's 3-frame pipeline — we drop the separate RHI thread since wgpu already abstracts the graphics API.

While the Render Thread draws frame _N_, the Game Thread is already computing frame _N+1_. The extract step at the end of each game tick is the synchronization point.

- **Step 1: Game Thread (Frame N).** The engine processes game logic, physics, animation, and input. At the end of the tick, the extract step diffs the World against the `ChangeTracker` and produces a `FramePacket` — views, lights, resource operations, and per-backend scene deltas — and sends it through a bounded channel.
- **Step 2: Render Thread (Frame N, one step behind).** The Render Thread receives the `FramePacket`, applies its deltas to the retained `RenderScene`, processes GPU resource updates, then executes the render graph to produce the final image.

This introduces one frame of input latency (~16 ms at 60 fps) in exchange for up to double the throughput when both threads carry comparable load. Same tradeoff as UE5.

```
time ──────────────────────────────────────────────────────▶

Game:    │ update(N) │ extract │ update(N+1) │ extract │ ...
         │           │  send ──┐             │  send ──┐
         │           │         │             │         │
Render:  │ render(N-1)         │ render(N)             │ render(N+1)
         │                     └─recv                  └─recv
```

### Thread Model

The execution contexts, each with clear ownership boundaries:

| Context                                              | Owns                                                                                          | Communicates via                                                                                                               |
| ---------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| **Game Thread** (main)                               | World, Input, Time, Systems, AssetServer                                                      | Sends `FramePacket` (data) + lifecycle control channel (resize, quit — OQ 15) to Render Thread; receives `FrameFeedback` (N-2) |
| **Render Thread** (dedicated)                        | GpuContext, RenderScene (retained draw data), RenderGraph, GPU resource pools, render targets | Receives `FramePacket`; sends `FrameFeedback` back                                                                             |
| **Worker Pools** (two: game + render, work-stealing) | Nothing persistent — borrow work items                                                        | Scoped tasks with `join()` / `parallel_for()`; split prevents priority inversion (OQ 16)                                       |
| **Streaming Coordinator** (dedicated, low-priority)  | Demand priority queue, budget arbiter (OQ 17)                                                 | Demand channel in (Game); `UploadBatch` channel out (Render); dispatches tasks onto the worker/IO pools                        |
| **Audio Thread** (dedicated)                         | Mixer, voices, output stream                                                                  | Drains `AudioCommands` each frame                                                                                              |
| **IO Pool** (blocking IO + decode)                   | Nothing persistent                                                                            | Tasks from AssetServer and Streaming Coordinator; decodes directly into staging regions (OQ 5)                                 |

The worker pools are split (OQ 16): the **game pool** runs physics (the provider's internal parallelism binds here), animation sampling, and streaming demand computation; the **render pool** runs frustum culling, draw call sorting, and batch generation. The split prevents priority inversion — render-critical culling never queues behind physics islands — and costs little utilization because the 2-frame pipeline keeps both pools concurrently busy. A unified task-graph scheduler with declared dependencies and priorities is the v2 evolution (see Physics — Worker-Pool Split).

### Render-to-Game Feedback

The pipeline is not one-directional. The Render Thread sends a `FrameFeedback` back to the Game Thread after each frame is submitted. This travels through a separate channel — the Game Thread typically reads feedback from frame N-2 while processing frame N. Never a synchronous wait.

Feedback data has two ages. CPU-side data (cull statistics) describes the frame the feedback was sent after. GPU-derived data (timestamps, compute readbacks) is older: at submit time the GPU has not yet executed the frame, so query results are collected through a **frames-in-flight readback ring** — 2–3 buffered query/readback sets, polled via `map_async` without blocking — and each GPU-derived datum is stamped with the frame it actually measures. The Render Thread never blocks on the GPU to assemble feedback.

```
time ──────────────────────────────────────────────────────▶

Game:    │ update(N)          │ update(N+1)          │ update(N+2)
         │ reads feedback(N-2)│ reads feedback(N-1)  │ reads feedback(N)
         │           send ──┐ │            send ──┐  │
         │                  │ │                   │  │
Render:  │ render(N-1)      │ │ render(N)         │  │ render(N+1)
         │ feedback(N-1) ───┘ │ feedback(N) ──────┘  │
```

```rust
struct FrameFeedback {
    frame_index:    u64,                       // frame this feedback was sent after
    gpu_time:       Option<GpuTimingFeedback>, // from the readback ring; None until first results land
    occlusion:      OcclusionFeedback,         // CPU cull stats for frame_index
    readback:       Vec<ReadbackResult>,
}

struct GpuTimingFeedback {
    measured_frame:  u64,                 // the frame these numbers describe (≥ ring depth behind frame_index)
    total_gpu_ms:    f32,
    pass_timings:    Vec<(PassId, f32)>,  // PassId: interned pass name, assigned at graph build
    gpu_memory_used: u64,
}

struct OcclusionFeedback {
    visible_mesh_count:   u32,
    visible_volume_count: u32,
    culled_count:         u32,
}

enum ReadbackResult {
    // Generic tagged readback — pick results, exposure, debug captures. (A per-entity
    // hardware-occlusion-query variant was cut: culling is HZB-based; nothing consumed it.)
    ComputeResult { source_frame: u64, tag: u32, data: Vec<u8> },
}
```

The Game Thread accesses this through `GameContext`:

```rust
impl GameContext<'_> {
    fn feedback(&self) -> Option<&FrameFeedback>;
    fn gpu_frame_time(&self) -> f32;  // convenience: latest feedback's total_gpu_ms
}
```

Because feedback is always from a past frame, game code must treat it as advisory — useful for adaptive quality (drop LOD if GPU is overloaded), streaming priority (don't stream what's culled), and profiling, but never as ground truth for the current frame's state.

### Controlling Pipeline Depth

The 2-frame pipeline is the default. For latency-critical applications (VR, competitive multiplayer), the engine can be configured to synchronize the threads, reducing to a 1-frame pipeline at the cost of throughput:

```rust
EngineConfig {
    pipeline_mode: PipelineMode::Overlapped,  // default: 2-frame
    // PipelineMode::Lockstep               // 1-frame, lower latency
}
```

In lockstep mode, CPU-side feedback arrives from the immediately preceding frame instead of N-2 (GPU-derived data still trails by the readback ring depth), but the game thread stalls until the render thread finishes — the same bottleneck UE5 documents with `r.OneFrameThreadLag 0`.

### Frame Pacing & Latency Control

_(OQ 8 resolution, 2026-08-11.)_ Three control loops, all consuming machinery that already exists — `GpuTimingFeedback`, the readback ring's completion tracking, and `ViewParams.resolution_scale`. No new cross-thread channels.

1. **GPU queue-depth throttle (v1).** The Render Thread waits on the GPU-completion signal for frame N−1 before submitting N+1, capping GPU frames in flight at `max_gpu_frames_in_flight` (default 1). Worst-case input latency becomes bounded and deterministic instead of driver-dependent — the Maximum Frame Latency analog. A correctness floor with no downside.
2. **Dynamic resolution controller (v1).** Consumes GPU frame time vs. `target_frame_time` and adjusts `resolution_scale` within `[min_scale, 1.0]`: **asymmetric response** (drop resolution fast on overrun, recover slowly), a **hysteresis band** (no oscillation around the target), **step-limited** changes (TAAU history stays valid). Strategic effect: the GPU stays inside budget, so loop 1 rarely engages and pacing stays smooth rather than reactive.
3. **Predictive tick pacing (v2 — `LatencyMode::LowLatency`, game-tunable).** Delays the game tick so input sampling happens as late as possible: a computed sleep before the INPUT phase from predicted GPU time + safety margin (Reflex-style). Tuning-sensitive — an optimistic margin misses vsync — hence flag-gated, with margins exposed to games, shipped once real content exists to calibrate against.

**Vsync & present modes.** `PacingConfig.vsync` maps to wgpu present modes: `true` → `Fifo` (vblank-paced, universal), `false` → `Mailbox` if available else `Immediate`. Vsync paces the _whole pipeline_ through designed backpressure — present blocks the Render Thread, the bounded packet channel fills, the Game Thread blocks on send — while the queue-depth throttle (loop 1) keeps the driver from hiding 2–3 frames of latency inside that blocking. The simulation is untouched: the fixed accumulator absorbs any render cadence. A **vsync-off frame cap** is a software limiter in the pacing module — sleep-to-target at tick start against `target_frame_time_ms` (v1-trivial; v2's predictive pacing subsumes it). Runtime toggles ride `set_pacing()` → surface reconfigure at a frame boundary, the same path as resize.

`PipelineMode::Lockstep` remains the blunt instrument for genuinely latency-critical applications; with these loops, Overlapped mode is latency-competitive for everything else.

```rust
struct PacingConfig {
    vsync:                    bool,
    target_frame_time_ms:     Option<f32>,  // None = display refresh interval
    max_gpu_frames_in_flight: u8,           // default 1
    drs:                      DrsConfig,
    latency_mode:             LatencyMode,
}

struct DrsConfig { enabled: bool, min_scale: f32 }

enum LatencyMode {
    Standard,    // v1: queue-depth throttle + DRS
    LowLatency,  // v2: + predictive tick pacing (game-tunable margins)
}
```
