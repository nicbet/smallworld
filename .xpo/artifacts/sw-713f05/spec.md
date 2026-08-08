## What

Wire `Material.emissive` (Vec3) through to the GPU so emissive surfaces render as self-lit. Add `emissive_map: Option<TextureKey>` for per-texel emissive variation. Add `KHR_materials_emissive_strength` extension support for HDR emissive values beyond [0,1]. The final emissive contribution is `emissive_texture * emissive_factor * emissive_strength`, added to the output color after all lighting — emissive bypasses shadows, attenuation, and BRDF.

## Why

Emissive is a core PBR material property. Without it, glowing surfaces (lava, neon signs, HUD elements, magic effects) can't be expressed. The CPU-side `Material.emissive` field already exists and the GLB loader reads `emissive_factor`, but none of it reaches the GPU. This completes the material pipeline for emissive.

## Acceptance Criteria

1. Materials with non-zero `emissive` render visibly brighter than ambient, independent of scene lighting
2. `emissiveTexture` from glTF/GLB files loads and applies per-texel emissive color
3. `emissive_output = emissive_texture_sample * emissive_factor * emissive_strength` — texture modulates the factor, strength scales it
4. Materials with `emissive: Vec3::ZERO` and no emissive texture behave identically to current rendering (no visual regression)
5. Fallback: when no emissive texture is bound, the shader uses a white fallback (so scalar `emissive` works alone)
6. Sandbox scene includes at least one emissive surface for visual verification
7. `KHR_materials_emissive_strength` extension is read and applied

## Flow

### Step 1: Add `emissive_map` to Material

**File:** `crates/engine/src/material.rs`

- Add `emissive_map: Option<TextureKey>` field to `Material`
- Default to `None`

### Step 2: Add emissive to `DrawUniforms`

**Files:** `crates/engine/src/gbuffer.rs`, `crates/engine/shaders/gbuffer.wgsl`, `crates/engine/src/lighting.rs`, `crates/engine/shaders/shadow.wgsl`

Add `emissive: [f32; 4]` (.xyz = emissive color, .w = pad) after the existing `_pad` field. Grows DrawUniforms from 96 → 112 bytes.

### Step 3: Add emissive GBuffer render target

**File:** `crates/engine/src/gbuffer.rs`

- Create `gbuf_emissive` texture — format `Rgba16Float` (supports HDR emissive > 1.0)
- Add `emissive_view` to the `GBuffer` struct
- Add as 4th color attachment in the render pass (`@location(3)`)
- Add as 4th pipeline color target

### Step 4: Emissive texture in bind group

**File:** `crates/engine/src/gbuffer.rs`

- Expand `tex_bind_group_layout` from 4 entries to 5: albedo(0), normal(1), roughness_metallic(2), emissive(3), sampler(4)
- Add `fallback_emissive` to `TextureCache`: white `[255, 255, 255, 255]`
- Update per-draw bind group creation to include emissive texture view

### Step 5: GBuffer shader writes emissive

**File:** `crates/engine/shaders/gbuffer.wgsl`

- Add `t_emissive` at binding 3, sampler moves to binding 4
- Add `@location(3) emissive: vec4<f32>` to `GBufferOutput`
- Sample: `emissive_sample.rgb * draw.emissive.xyz`

### Step 6: Shade pass reads emissive

**Files:** `crates/engine/src/lighting.rs`, `crates/engine/shaders/shade.wgsl`

- Bind `gbuf_emissive` at binding 5 in the shade bind group
- Read in shader: `let emissive = textureLoad(gbuf_emissive, pixel, 0).rgb;`
- After lighting loop: `color += emissive;`

### Step 7: GLB loader reads emissive texture + strength

**File:** `crates/engine/src/assets.rs`

- Add `emissive: Option<usize>` to `MaterialTextures`
- Read `mat.emissive_texture()` in `extract_texture_indices()`
- Read `mat.emissive_strength()` (KHR_materials_emissive_strength), multiply into emissive factor
- Wire emissive texture key in `LoadedScene::spawn()`

### Step 8: Enable gltf crate feature

**File:** `Cargo.toml`

- Enable `KHR_materials_emissive_strength` feature on the `gltf` dependency

### Step 9: Sandbox verification

**File:** `crates/sandbox/src/main.rs`

- Add neon-green emissive cube with HDR emissive `(0, 3, 1.5)`

## Decisions

- **4th GBuffer target (Rgba16Float), not packing into material.ba** — the material target is `Rgba8Unorm` with only 2 unused channels at 8-bit precision. Emissive needs 3 channels and HDR range. A dedicated target is the standard deferred rendering approach.
- **White fallback texture for emissive, not black** — white means `fallback * emissive_factor = emissive_factor`, so scalar-only emissive works.
- **Emissive added after lighting, not before** — emissive is unlit by definition. `color += emissive` after the lighting loop is the correct PBR formulation.
- **KHR_materials_emissive_strength** — multiplied into the emissive factor at load time, not stored separately. The shader doesn't need to know about the extension.
