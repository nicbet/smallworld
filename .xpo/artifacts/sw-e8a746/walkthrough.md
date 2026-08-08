# Walkthrough: All-white roughness/metallic fallback

## What was built

A one-line fix in `TextureCache::new` (`crates/engine/src/gbuffer.rs`): the 1×1 fallback texture bound when a material has no roughness/metallic map changed from `[255, 128, 0, 255]` to all-white `[255, 255, 255, 255]`.

## Why

The GBuffer shader combines texture and scalar multiplicatively, following the glTF convention (`gbuffer.wgsl`):

```wgsl
let roughness = rm_sample.g * draw.roughness_metallic.x;
let metallic  = rm_sample.b * draw.roughness_metallic.y;
```

Under a multiplicative convention, the "no texture" fallback must be the multiplicative identity — 1.0 in every sampled channel. The old fallback had B = 0, so `metallic` was forced to 0 for every untextured material (the sandbox metal cube rendered as a dielectric), and G = 128 silently halved `roughness`.

## How the pieces fit

The fallback family in `TextureCache` now reads:

- `fallback_albedo` — white (multiplicative identity) ✓ already correct
- `fallback_normal` — `[128, 128, 255, 255]` (flat tangent-space normal — the *additive* identity for normal mapping, deliberately not white) ✓ unchanged
- `fallback_rm` — white (this fix)
- `fallback_emissive` — white ✓ already correct

`view_or_fallback` binds these per-draw whenever the material lacks the corresponding map, so the fix applies uniformly to every untextured draw with zero runtime cost.

## Non-obvious context for future readers

A correctly-metallic surface in the current renderer looks near-black with sharp highlights: `metallic = 1` zeroes the diffuse term and specular can only reflect the analytic lights — there is no environment map yet. Don't mistake that dark appearance for this bug recurring. Making metals *read* as metal requires environment reflection (proposed as a follow-up procedural-environment feature, not yet filed).
