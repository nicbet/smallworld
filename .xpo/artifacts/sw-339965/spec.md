# Spec: Godot-style receiver-side shadow bias

## What

Replace hardware depth bias on the shadow pipeline with receiver-side biasing in `sample_shadow` (`shade.wgsl`), following Godot's scheme.

## Why

Hardware slope-scale bias under-biases at grazing angles (acne) and over-biasing causes Peter-Panning. Godot's receiver-side scheme avoids both: the normal offset moves the sample point across the shadow map but — because the light-parallel component is projected out — can never alter the depth comparison, so it cannot detach shadows. Depth separation is handled independently by a small constant bias along the light direction.

## How

**`crates/engine/src/lighting.rs`** — shadow pipeline:
- `DepthBiasState { constant: 0, slope_scale: 0.0, clamp: 0.0 }` (was 2 / 2.0)
- Keep `cull_mode: Back` (matches Godot — no front-face-culling trick)

**`crates/engine/shaders/shade.wgsl`** — `sample_shadow` gains `normal` and `l` (surface→light dir) parameters:

1. **Constant depth bias along light**: `pos = world_pos + l * SHADOW_DEPTH_BIAS` — moves the receiver toward the light, shrinking its depth in light space.
2. **Slope-scaled normal offset**: `offset = normal * texel_world_size * SHADOW_NORMAL_BIAS * (1.0 - saturate(dot(normal, l)))` — zero at normal incidence, maximal at grazing.
3. **Project out the light-parallel component**: `offset -= l * dot(l, offset)` — offset now only shifts shadow-space XY, never depth (the anti-Peter-Pan detail).
4. Project `pos + offset` by the shadow view-proj and compare as before.

Texel world size is derived in-shader from the ortho view-proj matrix: the world-space width of the frustum is `2.0 / length(row0(view_proj).xyz)`, so `texel_world_size = width / viewport_px`. Exact for orthographic (the only shadow projection today); adapts automatically when CSM changes cascade extents later.

## Constants (tunable)

- `SHADOW_NORMAL_BIAS = 2.0` (texels) — Godot's `shadow_normal_bias` default
- `SHADOW_DEPTH_BIAS = 0.05` (world units) — small constant; Godot's directional default is 0.1, halved here since the test scene is small

## Agent decisions

- **Texel size derived in-shader** instead of adding a field to `ShadowView` — avoids touching the Rust-side GPU struct layout; revisit if perspective (spot/omni) shadow views are added, which need per-w texel scaling.
- **Bias constants are shader constants**, not per-light material params — per-light tuning can be added when spot/omni shadows land.

## Acceptance criteria

- [ ] No visible acne stripes on lit surfaces under the directional light
- [ ] No Peter-Panning — shadows stay attached at cube bases
- [ ] `cargo build` + existing tests pass
