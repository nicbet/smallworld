## Emissive Material Support (Scalar + Texture)

### What was built

Emissive materials are now fully wired through the GPU pipeline. A surface with non-zero `Material.emissive` renders as self-lit — its emissive radiance is added to the final color after all lighting, bypassing shadows, attenuation, and BRDF. Emissive textures from glTF models modulate the scalar factor per-texel. The `KHR_materials_emissive_strength` extension is supported for HDR emissive values beyond the [0,1] base spec range.

### How the pieces fit together

**Material struct** (`crates/engine/src/material.rs`)
- New field: `emissive_map: Option<TextureKey>` — references an emissive texture in the World's texture store.

**DrawUniforms** (`gbuffer.rs`, `lighting.rs`, `gbuffer.wgsl`, `shadow.wgsl`)
- Added `emissive: [f32; 4]` (.xyz = emissive color, .w = pad). Grows the struct from 96 → 112 bytes. All four copies (two Rust, two WGSL) are kept in sync. The shadow pass carries the field for layout compatibility but writes zeros.

**GBuffer** (`crates/engine/src/gbuffer.rs`)
- 4th render target: `gbuf_emissive` at `Rgba16Float`. HDR format preserves emissive values > 1.0 for future bloom. Added as `@location(3)` in both the pipeline color targets and the render pass color attachments. Cleared to black.
- `GBuffer` struct gains `emissive_view: wgpu::TextureView`.

**Texture bind group** (`crates/engine/src/gbuffer.rs`)
- Layout expanded from 4 → 5 entries: albedo(0), normal(1), roughness_metallic(2), **emissive(3)**, sampler(4). Sampler moved from binding 3 to 4.
- `TextureCache` gains `fallback_emissive`: a white 1×1 texture. White fallback means `white * emissive_factor = emissive_factor`, so scalar-only emissive works without a texture map. Black fallback would have zeroed everything out.
- Per-draw bind group creation includes the emissive texture view (or fallback).

**GBuffer shader** (`crates/engine/shaders/gbuffer.wgsl`)
- `t_emissive` sampled at binding 3, multiplied by `draw.emissive.xyz`.
- Output written to `GBufferOutput.emissive` at `@location(3)`.

**Shade pass** (`crates/engine/src/lighting.rs`, `crates/engine/shaders/shade.wgsl`)
- `gbuf_emissive` bound at `@group(0) @binding(5)` in the shade compute bind group (after hdr_output at binding 4, to avoid renumbering existing bindings).
- Shader reads emissive via `textureLoad` and adds it to `color` after the lighting loop: `color += emissive`. This is correct because emissive is unlit by definition — it doesn't interact with lights or shadows.

**GLB loader** (`crates/engine/src/assets.rs`)
- `MaterialTextures` gains `emissive: Option<usize>`.
- `extract_texture_indices()` reads `mat.emissive_texture()`.
- `extract_material()` reads `mat.emissive_strength()` from the `KHR_materials_emissive_strength` extension (defaults to 1.0) and multiplies it into the emissive factor at load time. The shader doesn't need to know about the extension.
- `LoadedScene::spawn()` wires `emissive_map` from texture indices.

**Cargo.toml**
- `gltf` dependency gains `KHR_materials_emissive_strength` feature.

**Sandbox** (`crates/sandbox/src/main.rs`)
- Neon-green emissive cube: `emissive: Vec3(0, 3, 1.5)`, dark base color, high roughness. Renders as bright self-lit cyan, visibly brighter than all other objects in the scene. Validates that HDR emissive survives through the Rgba16Float GBuffer and tone-maps correctly.

### Key decisions

- **Dedicated GBuffer target over packing into material.ba** — the material target is Rgba8Unorm with only 2 spare channels at 8-bit precision. Emissive needs 3 channels and HDR range. A 4th target at Rgba16Float is the standard deferred rendering approach.
- **White fallback, not black** — the critical invariant for scalar-only emissive to work.
- **emissive_strength baked into the factor at load time** — simpler than carrying a separate multiplier through the uniform/shader pipeline.
- **Binding 5 in shade pass** — placed after hdr_output (binding 4) rather than renumbering, to minimize disruption to existing bind group layout.

### Follow-up

sw-e6767a filed under E7 for emissive surfaces actually emitting light (auto point lights or GI). This issue delivers the appearance pipeline only.
