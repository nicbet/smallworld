# Packages & Allowed Dependencies

Crate/module overview. Arrows point from consumer to dependency; the game–render firewall and
the plugin thesis are visible as _which arrows do not exist_ — game code never reaches `render`
or `wgpu`, and plugins consume only public engine contracts.

@import "00-packages.mmd" {as="mermaid"}

**Firewall check (arrows that must never appear):** `SANDBOX → RENDER`, `SANDBOX → WGPU`,
`WORLD → WGPU`. **Plugin check:** `VOXEL` reaches only public contracts in `RENDER`/`STREAM` —
never engine internals.
