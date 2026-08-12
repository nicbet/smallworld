## Critical Path & Build Order

_(Planning, 2026-08-11.)_ The absolute barebones for end-to-end: a game uses the engine, pixels
reach the screen, game logic runs. Everything else branches off naive implementations.

**The golden rule: naive implementations must be contract-shaped.** Every improvement track
below replaces _internals behind an interface that already exists_ — a lighting slot gets fed,
a provider gets swapped, a tier gets added. The skeleton therefore adopts four contracts on day
one, in naive form, because they are cheap now and world-rewrites later:

1. **f64 `Transform.position` + the per-cell offset plumbing** (OQ 30) — with a single implicit
   cell at the origin until streaming arrives. Retrofitting `DVec3` into a shipped World is the
   one migration this document refuses to schedule.
2. **The instance lane** (`DrawId`/`InstanceSlot` allocation, `InstanceData` with fade+flags) —
   PickId, LOD fades, and foliage all stand on slot stability.
3. **The GBuffer contract as specified** — including the shading-model/flag bits in albedo
   alpha and the velocity target (written even before TAA consumes it).
4. **Profiling lanes from the first commit** (OQ 24) — the macros are nearly free, and every
   subsequent milestone is validated through them.

### The Spine (critical path)

- **M0 — Boot.** Window + event loop; `GpuContext` + `Capabilities` probe; the two threads with
  packet, feedback, and control channels; `Engine::run` / `App` / `Time`; profiling lanes.
- **M1 — First pixels.** Minimal World (`Transform`, `MeshRenderer`, `Camera`, `LightSource`);
  `ChangeTracker` → delta extract → retained `RenderScene` (mesh store + instance lane);
  `StagingRef` over a naive one-buffer pool; GPU pools; minimal render graph executing
  GBuffer → lighting (one hardcoded sun, no shadows) → ACES tonemap → present. _No depth
  pre-pass, no HZB, no TAA — all optimizations, none required for correctness._
- **M2 — A game.** Input; `fixed_update` + accumulator + the interpolation contract;
  double-buffered events; Rust-tier behaviors in the `BehaviorHost`; systems + phases.
  A player-controlled entity moves under game logic: the engine/game split is real.
- **M3 — Survives reality.** Resize + staged teardown (a demo that dies on resize is not a
  demo); direct glTF/PNG import at load (the cook cache comes later, behind the same
  `AssetServer` API); egui overlay with the budget table.

After M3 the spine ends; everything else is a branch track with its own naive → improved → v2
chain, joined only by the contracts.

### Build-Order Graph

```mermaid
flowchart TD
    M0["M0 · Boot<br/>window · GpuContext · Capabilities<br/>threads + channels · App/Time<br/>profiling lanes (day one)"]
    M1["M1 · First pixels<br/>World (f64 pos) · delta extract · retained scene<br/>instance lane · naive StagingRef · GPU pools<br/>GBuffer → sun light → ACES → present"]
    M2["M2 · A game<br/>Input · fixed tick + interpolation<br/>events · Rust behaviors · systems"]
    M3["M3 · Survives reality<br/>resize + teardown · direct glTF/PNG<br/>egui overlay + budget table"]
    M0 --> M1 --> M2 --> M3

    subgraph LIGHT["Lighting"]
        L1["clustered lights"] --> L2["shadow atlas (CSM, shadow views)"] --> L3["env capture / IBL<br/>+ sky visibility floor"] --> L4["froxel volumetrics"] --> L5["GI clipmap<br/>(+ sky-vis cones)"] --> L6["RT passes (cap-gated,<br/>clipmap hit radiance)"] --> L7["v2/3 · Lumen analog"]
    end

    subgraph POST["Post / AA"]
        P1["TAA (velocity + jitter)"] --> P2["TAAU + internal/display split"] --> P3["auto-exposure"] --> P4["DRS"] --> P5["v2 · FSR 2.2 / DLSS slot"]
    end

    subgraph PACE["Pacing"]
        PC1["queue-depth throttle"] --> PC2["DRS controller"] --> PC3["v2 · predictive pacing"]
    end

    subgraph SCALE["World scale"]
        S1["hierarchy + LATE systems"] --> S2["World Streaming cells<br/>+ OQ20 documents"] --> S3["Streaming Coordinator<br/>+ detail streaming + staging rings"] --> S4["multi-cell LWC anchors"] --> S5["v2 · GPU-driven phase 2"]
        S2 --> SER1["save games<br/>(registry + EntityRef)"]
    end

    subgraph VOX["Voxel Plugin"]
        VP1["VolumeSource (code) + brick tree/pool"] --> VP2["VolumePass raymarch (near)"] --> VP3["MC+Transvoxel far tier + handoff"] --> VP4["materials + dither"] --> VP5["density graphs"] --> VP6["edits + fan-out"] --> VP7["collision ring"] --> VP8["PCG scatter + foliage"]
    end

    subgraph PHYS["Physics"]
        PH1["rapier provider<br/>fixed tick + sync-back"] --> PH2["queries + CPU picking"] --> PH3["joints"] --> PH4["character controller"]
    end

    subgraph ANIM["Animation"]
        A1["DeformPass + clip sampling"] --> A2["AnimGraph + state machines"] --> A3["events · root motion · sockets"] --> A4["IK nodes"]
    end

    subgraph AUD["Audio"]
        AU1["cpal + voices + commands"] --> AU2["mixer buses (data)"] --> AU3["streaming music"] --> AU4["reverb/DSP → v2 graphs"]
    end

    subgraph ASSET["Asset pipeline"]
        AS1["derived-data cache<br/>(cook-on-demand)"] --> AS2["GUIDs + .meta + VFS mounts"] --> AS3["dependency closures"] --> AS4["ship cook + paks"]
    end

    subgraph SCRIPT["Scripting & UI"]
        B1["Lua tier (mlua, accessor API)"] --> B2["C ABI vtable"]
        UI1["game-UI round<br/>(widgets = entities)"] --> UI2["editor (application, far)"]
    end

    subgraph SKY["Sky & weather"]
        W1["Hillaire atmosphere"] --> W2["clouds module"] --> W3["WeatherState + hooks"]
    end

    M3 --> L1
    M3 --> P1
    M3 --> PC1
    M3 --> S1
    M3 --> A1
    M3 --> AS1
    M2 --> PH1
    M2 --> AU1
    M2 --> B1
    M3 --> UI1
    L3 --> W1
    S3 --> VP1
    L2 -.->|"ShadowCaster"| VP2
    L5 -.->|"GI injection"| VP6
    M1 -.->|"velocity target ready since M1"| P1
    PC2 -.->|"consumes resolution_scale"| P2
    S4 -.->|"before planet content"| VP1
    PH2 -.->|"gameplay raycasts"| VP7
    VP8 -.->|"ScatterSurface"| S2
```

### Sequencing Notes

- **The Voxel Plugin branches off streaming, not off rendering** — its residency, sources, and
  cell integration are streaming-shaped; its render passes then plug into contracts M1 already
  built. Multi-cell LWC anchors (S4) must land before planet-scale content, not before voxels
  as such.
- **Physics and audio branch off M2** (they serve game logic), rendering tracks off M3.
- **Shadow views (L2) are the multi-view forcing function** — the first consumer of per-view
  culling beyond the main camera; build them before any aux-view feature (probes, RTT).
- **v2 items** (Lumen analog, GPU-driven, FSR, predictive pacing, audio graphs, editor) hang
  off the ends of their tracks — none is load-bearing for any other track's v1, by
  construction (the Deferred Ledger's category-B rounds).
