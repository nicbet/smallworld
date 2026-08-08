## What Was Built

Material texture map support: albedo, normal, and roughness/metallic maps. The engine can now render textured GLB models with full PBR materials instead of flat scalar colors.

## Architecture

### New module: `texture.rs`

`TextureData` — CPU-side RGBA8 pixel storage (Vec<u8>, width, height). Stored in World via `TextureKey` (SlotMap). Immutable after creation, no change tracking.

### Material changes (`material.rs`)

Three new `Option<TextureKey>` fields:
- `albedo_map` — sampled and multiplied with `base_color`
- `normal_map` — tangent-space normal, transformed by TBN matrix
- `roughness_metallic_map` — green=roughness, blue=metallic (glTF convention), multiplied with scalar values

All default to `None`. Existing scalar-only materials work unchanged via `..Material::default()`.

### GPU texture cache (`gbuffer.rs`)

`TextureCache` maintains a `HashMap<TextureKey, GpuTextureEntry>` with lazy upload — textures are uploaded to the GPU on first use via `queue.write_texture`. Three 1×1 fallback textures created at init:
- White (1,1,1,1) for albedo — multiplying with base_color preserves the scalar value
- Flat normal (0.5, 0.5, 1.0) — decodes to (0,0,1) in tangent space, which TBN transforms to the vertex normal
- Default RM (1.0, 0.5, 0.0) — roughness=0.5, metallic=0.0 when multiplied with scalar defaults

### Bind group changes

GBuffer pipeline gains a third bind group (group 2):
- binding 0: albedo texture
- binding 1: normal map texture
- binding 2: roughness/metallic texture
- binding 3: shared sampler (bilinear + trilinear mip)

Per-draw: a new bind group is created with the material's texture views (or fallbacks). The pipeline layout now has three bind group layouts.

### Shader changes (`gbuffer.wgsl`)

Vertex output extended with `world_tangent`, `tangent_w`, and `uv` passthrough.

Fragment shader:
1. **Albedo**: `textureSample(t_albedo, t_sampler, uv) * base_color`
2. **Normal**: samples normal map, decodes [0,1]→[-1,1], transforms by TBN matrix (built from vertex normal + tangent + bitangent). `@builtin(front_facing)` flips the normal for back-faces before TBN construction.
3. **Roughness/metallic**: `green_channel * roughness_scalar`, `blue_channel * metallic_scalar`

### GLB loader updates (`assets.rs`)

`gltf::import` now captures the third return value (images). Each image is converted to RGBA8 (handling R8G8B8→R8G8B8A8 conversion). `MaterialTextures` struct stores indices into the texture array per material slot. `LoadedScene::spawn` uploads textures to World, then wires `TextureKey`s into Materials.

`extract_texture_indices` reads glTF texture references:
- `pbr.base_color_texture()` → albedo
- `mat.normal_texture()` → normal map
- `pbr.metallic_roughness_texture()` → roughness/metallic

### Sandbox changes

GLB scene lighting changed from single harsh directional to three-point setup (key + fill + rim) for better model viewing.

## Key Decisions

**Lazy GPU upload.** Textures are uploaded on first render use, not at load time. This keeps `load_glb` free of GPU dependencies and lets the World store CPU-side data that the rendering pipeline manages.

**`GBufferPass::new` takes `&Queue`.** Needed to create fallback textures at init. Minor API change, only affects `Engine::new`.

**Per-draw bind groups.** Each draw call gets a fresh bind group with its material's textures. Not optimal (bind group caching by material would be better) but correct and simple. Optimization is future work.

**Fallback design.** The shader always samples all three textures. When a material has no texture, the fallback produces the identity value for that operation (white for multiply, flat for normal, 1.0 for roughness/metallic multiply). No shader branching needed.
