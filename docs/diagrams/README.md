# Architecture Diagrams

UML (Mermaid) views of the **target architecture** as specified in `../architecture-design.md`
and `../voxel-plugin-design.md`. These diagrams describe the design, not the current source
(which is mid-M0); when the two disagree, the design documents win.

| File | View |
|------|------|
| [00-packages.md](00-packages.md) | Package/crate overview and allowed dependencies |
| [01-core-runtime.md](01-core-runtime.md) | Engine core: `App` lifecycle, threads, channels, pacing |
| [02-world-gameplay.md](02-world-gameplay.md) | World/ECS, components, events, behavior model |
| [03-rendering.md](03-rendering.md) | Extraction & backends; render graph & GPU resources |
| [04-assets-streaming.md](04-assets-streaming.md) | Asset pipeline, VFS, staging, streaming, serialization |
| [05-physics-animation-audio.md](05-physics-animation-audio.md) | Simulation subsystems |
| [06-voxel-plugin.md](06-voxel-plugin.md) | The Voxel Plugin on the public contracts |
| [07-game-engine-flow.md](07-game-engine-flow.md) | **Start here** — ownership, the one-time handoff, the game loop, the frame's data journey, scene changes & pause |

Conventions: Rust traits are stereotyped `<<trait>>`, enums `<<enumeration>>`, plain-data
components `<<component>>`, serde data-graph assets `<<asset>>`. Only the most important public
methods are shown; signatures are simplified (no lifetimes, references, or error types).
