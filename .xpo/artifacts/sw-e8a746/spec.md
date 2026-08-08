# Spec: All-white roughness/metallic fallback texture

## What

Change the 1×1 fallback roughness/metallic texture in `TextureCache` (`crates/engine/src/gbuffer.rs`) from `[255, 128, 0, 255]` to `[255, 255, 255, 255]`.

## Why

`gbuffer.wgsl` multiplies sampled texture channels by the material scalars (glTF convention: texture × factor). A neutral fallback must be the multiplicative identity — all-white — so untextured materials get exactly their scalar `roughness`/`metallic` values. The current fallback zeroes metallic (B=0) and halves roughness (G=128).

## How

One-line change to the `create_1x1_texture` call for `fallback_rm`. The albedo and emissive fallbacks are already all-white (correct); the normal fallback `[128, 128, 255, 255]` is the correct flat-normal identity and stays.

## Acceptance criteria

- [ ] An untextured material with `metallic: 1.0` shades as a metal (F0 = albedo, no diffuse term)
- [ ] An untextured material's roughness matches its scalar exactly
- [ ] Materials with an assigned RM texture are unaffected
- [ ] `cargo build` + existing tests pass
