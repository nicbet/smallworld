# Walkthrough: Godot-style receiver-side shadow bias

## What was built

Shadow-acne elimination for directional shadows by moving all biasing from the shadow-caster pass to the receiver, following Godot's scheme (verified against Godot source):

1. **`crates/engine/src/lighting.rs`** — the shadow pipeline's hardware `DepthBiasState` is now all zeros (was `constant: 2, slope_scale: 2.0`). Back-face culling is unchanged — Godot uses no front-face-culling trick.
2. **`crates/engine/shaders/shade.wgsl`** — `sample_shadow` now takes the surface `normal` and `to_light` direction and applies two independent receiver-side corrections before projecting into light space.
3. **`crates/engine/src/shaders.rs`** — new `shade_shader_validates` test compiling `shade.wgsl` on a headless device, mirroring `raymarch_shader_validates`. Previously WGSL errors in this shader only surfaced at first app launch.

## How the bias works

Two constants at the top of the shadow section in `shade.wgsl`:

- `SHADOW_NORMAL_BIAS = 2.0` (shadow-map texels) — Godot's default
- `SHADOW_DEPTH_BIAS = 0.05` (world units)

**Slope handling — normal offset.** The receiver position is offset along the surface normal by `texel_size × 2.0 × (1 − saturate(n·l))`: zero at normal incidence, maximal at grazing angles, which is exactly where acne is worst (one shadow texel spans a large depth range on a steep surface). The critical detail: the light-parallel component is then projected out (`offset -= to_light * dot(to_light, offset)`), so the offset moves the *sample position* across the shadow map but is geometrically incapable of changing the depth comparison. That is why this scheme cannot cause Peter-Panning, unlike naive normal-offset or large constant bias.

**Depth separation — constant bias.** Independently, the receiver is moved `0.05` world units *toward* the light before projection, shrinking its light-space depth slightly. This absorbs depth-quantization error without needing slope scaling (the normal offset already handled slope).

**Texel size** is derived in-shader from the shadow matrix: for an orthographic projection, NDC x spans the frustum width, so `|row0(view_proj).xyz| = 2 / width` and `texel_size = width / viewport_px`. No new GPU struct fields needed, and when cascaded shadow maps (sw-f967e8) change per-cascade extents, the bias adapts automatically.

## Key decisions

- **Texel size in-shader, not in `ShadowView`** — avoids a Rust/WGSL struct-layout change. Revisit when spot/omni shadow views land: perspective projections need the texel size divided by w.
- **Constants, not per-light parameters** — per-light bias tuning (Godot exposes it on each light) only becomes worthwhile with multiple shadow-casting light types.
- **Hardware bias fully zeroed rather than reduced** — mixing caster-side and receiver-side bias makes tuning circular; Godot ships with zeros and one scheme.

## Verification

96 tests pass, including the new shader-validation test. Visual check in the sandbox: no acne stripes on the floor or cube faces, shadows attached at the pillar and cube bases (no Peter-Panning).
