## Lifecycle

_(OQ 15 resolution, 2026-08-11.)_ Engine-level lifecycle: surface events, device loss, and shutdown.

### The Control Channel

Lifecycle events ride an **out-of-band control channel** (control/data-plane separation, per Gregory), main thread → Render Thread: `Resized`, `ScaleFactorChanged`, `Minimized/Restored`, `FocusGained/FocusLost`, `DeviceLost`, `Quit`. Focus events enable render throttling when the window loses input focus (e.g. alt-tab): the Render Thread caps its frame rate to reduce GPU/CPU usage while the game remains partially visible, and the Game Thread may optionally pause simulation depending on the game's policy. Control must be deliverable independent of packet flow — a paused game still resizes; a stalled pipeline still quits. **Transport is out-of-band; application is frame-synchronized:** the Render Thread drains the control channel at the top of RECEIVE and applies changes only between frames. Packets stamp the display size they were built against; a packet built pre-resize presents with scaling for that one transient frame. The data plane (`FramePacket`) never carries control.

### Resize

Main thread receives the window event, updates `WindowState` (game-visible), and sends `Resized` on the control channel. At the next frame boundary the Render Thread reconfigures the surface and reallocates display-resolution targets (TAAU history is display-res and resets on resize; internal-res maxima follow the new display size). `SurfaceError::Outdated`/`Lost` at acquire → reconfigure and retry once, else skip present that frame. There is no crash path through resize.

### Device Loss

**Invariant (architectural law): GPU memory is always a cache — no authoritative state lives only on the GPU.** This is already true by construction (retained `RenderScene` is CPU-side; bricks refill from region files/generators; assets re-load through the normal path; clipmap/froxels/histories/TLAS are transient and rebuild), and it is what keeps full recovery permanently possible.

- **v1: fatal with grace.** `DeviceLost` on the control channel → drain what is drainable, fire the game's save hook, emit diagnostics, exit through the teardown protocol.
- **Scheduled hardening: the recovery walk.** Pause the loop → recreate the device → recreate pools → re-request contents through the existing asset/streaming paths → transients rebuild over the next frames → resume. Additive, thanks to the invariant; deferred because device loss is rare and the test burden is the real cost.

### Teardown Protocol

Explicit staged shutdown — never `Drop`-order across five threads. Channel closure is the universal backstop signal; every stage has a deadline (~2 s) after which it is logged and forced. The process never hangs on exit.

1. **Stop simulation.** Exit the main loop; run `App::shutdown` and behavior `shutdown` callbacks **while all services still live** — World, AssetServer, streaming, IO — so saves flush through normal paths.
2. **Quiesce producers.** The streaming coordinator rejects new demand, mass-cancels its queue (generation stamps), **flushes pending region-file writes**, then dissolves.
3. **Drain the pipeline.** Close the packet and control channels; the Render Thread finishes in-flight frames and exits; GPU wait-idle; staging-pool and readback-ring fences complete; pools release.
4. **Stop services.** Audio drains and stops; worker pools join.
5. **Destroy.** GPU resources → device → window → process exit (`Engine::run() -> !` holds).

The ordering that bites: stage 1 before stage 2 — shutdown callbacks that save must run while the IO machinery is alive, not during destruction.
