## Frame Lifecycle

The complete sequence of a single frame, showing which thread owns each phase.

**Loop style, named:** an engine-owned **fixed-phase loop with minimal lifecycle hooks**
(template-method IoC — the Unity/Godot family), not `frameStarted`/`frameEnded` observer
callbacks (unordered listener lists are a bug farm; the structured substitute is a system
registered in the right `Phase`). Input is a **polled snapshot**, never pushed events — the
OS's callback pump (winit) is quarantined at the boundary by the INPUT phase. The `EventBus`
is a **data plane, never a control plane**: nothing in the loop is event-driven; events are
read during your phase, from last frame, deterministically. Between threads the style is
**CSP message passing** — the render/audio/streaming threads are receive-driven loops on owned
values. Every choice serves the same two masters: determinism (OQ 19 needs ordered phases) and
profilability (phases map one-to-one onto instrumentation lanes).

### Game Thread

| Phase       | Action                                                                                                                                                                            |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **INPUT**   | Main thread accumulates window events into `Input` snapshot                                                                                                                       |
| **FIXED**   | `App::fixed_update()` runs 0–N times at fixed timestep (0 while paused; paced by scaled time — OQ 33)                                                                             |
| **UPDATE**  | `App::update()` runs once. Game systems mutate World                                                                                                                              |
| **LATE**    | Engine systems: hierarchy propagation, bounds recomputation, streaming demand                                                                                                     |
| **EXTRACT** | Diff `&World` via `&ChangeTracker` into a `FramePacket` (views, lights, deltas, resource ops), interpolating fixed-tick transforms at the accumulator alpha; send through channel |
| **CLEAR**   | Clear change tracker. Game thread free for next frame                                                                                                                             |

### Render Thread

| Phase       | Action                                                                                                                           |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------- |
| **RECEIVE** | Drain control channel (resize/device events — frame-boundary application); block on packet channel, receive `FramePacket`        |
| **PREPARE** | Apply `ResourceOp`s and scene deltas — upload resources, update the retained `RenderScene`                                       |
| **CULL**    | Derive shadow views; collect TLAS instances (pre-cull); per-view frustum + occlusion culling; produce sorted, batched draw lists |
| **RECORD**  | Render graph executes: each pass records GPU commands                                                                            |
| **SUBMIT**  | Throttle on GPU queue depth (`max_gpu_frames_in_flight` — OQ 8), then `queue.submit()`                                           |
| **PRESENT** | Swapchain present. Send `FrameFeedback`. Loop back to RECEIVE                                                                    |

### Ownership Boundaries

| Data                                    | Owner                                | Crosses boundary as                                                                   |
| --------------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------- |
| World, Components                       | Game Thread                          | Read-only in extract                                                                  |
| FramePacket (deltas + views)            | Produced by Game, consumed by Render | Owned value through channel (Game → Render)                                           |
| RenderScene (retained draw data)        | Render Thread                        | Never — updated only by applying packet deltas                                        |
| FrameFeedback                           | Produced by Render, consumed by Game | Owned value through channel (Render → Game, ~2-frame lag; GPU data via readback ring) |
| Device-local GPU resources + submission | Render Thread                        | Never — game code uses handles                                                        |
| Staging buffers (CPU-visible, mapped)   | Engine staging pool (thread-safe)    | `StagingRef` through `ResourceOp`; fence-reclaimed after the GPU copy                 |
| Asset bulk data                         | AssetServer + staging pool           | Decoded directly into mapped staging off-thread; never copied by value                |
| Input                                   | Main Thread                          | Snapshot borrowed by game tick                                                        |
| Audio commands                          | Collected on Game Thread             | Drained by audio server each frame                                                    |
