## Atmosphere, Clouds & Weather

_(OQ 31 resolution, 2026-08-11.)_ Fog was already resolved by OQ 9 — global height fog in `EnvironmentParams`, `FogVolume` components, and the froxel system with its public injector; nothing there is reinvented per game. This section covers the sky half and weather. Engine/game split: the engine owns the models and the plumbing; games own the logic and drive engine state.

### Atmosphere — engine, physically based, planet-aware

`SkyMode::Procedural` is specified as **Hillaire's LUT-based scalable atmosphere** — the technique behind UE5's SkyAtmosphere and Unity HDRP's physically based sky: transmittance and multiple-scattering LUTs, cheap per frame. The decisive property for smallworld: **it is a planetary model** — correct from ground level through flight to space, which V2's radial worlds make a first-class case rather than an edge case. It feeds the existing IBL capture (time-of-day amortization already in place) and the froxel far field automatically. `SkyMode::Cubemap` remains the authored alternative.

### Clouds — engine module, scheduled

A **volumetric cloud layer**: Schneider-class noise-driven raymarching (the UE5 Volumetric Clouds shape) in its own altitude-slab raymarch pass — deliberately _not_ froxels, whose near-range grid is far too coarse for cloudscapes — lit by the atmosphere LUTs, with **cloud shadows** as a projected sun modulation riding the existing per-light shadow-mask slot. Scheduled after the v1 rendering core, like decals: purely additive, zero contract debt. Interim tier: panning cloud textures in the skybox path.

### Weather — state is engine, logic is game

- **The engine owns `WeatherState` and the plumbing.** Precipitation type and intensity, cloud coverage/darkness, wetness, snow cover, storm wind — driving _existing_ knobs: cloud parameters, froxel density boost, `EnvironmentParams::wind`, and **material response hooks**: global wetness/snow uniforms consumed by the `Standard` shading model (wetness darkens albedo and lowers roughness; snow blends by gravity-aligned slope — the V6 gravity frame again). Custom shading models opt in through the same uniforms.
- **Games own weather logic.** What weather occurs, when, and where is a behavior or system driving `WeatherState`, possibly per region. The engine never decides that it rains; it makes rain _representable_.
- **Precipitation rendering** is a GPU-instanced layer inside the weather module — camera-attached volume, depth-collided against the depth buffer, the standard shape. It deliberately does not wait for a general particle system (OQ 32).
