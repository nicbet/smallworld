## What

Add texture map support to the material system: albedo, normal, and roughness/metallic maps.

## Why

Without textures, the GLB loader shows flat white/gray surfaces. Real models rely on painted textures for color, surface detail, and material variation. The vertex format already has tangents for normal mapping.

## How

### 1. TextureData + TextureKey in World

```rust
pub struct TextureData {
    pub pixels: Vec<u8>,  // RGBA8
    pub width: u32,
    pub height: u32,
}
```

World gains `SlotMap<TextureKey, TextureData>` with `add_texture()` / `texture()`. Textures are immutable after creation.

### 2. Material gains texture slots

```rust
pub struct Material {
    pub base_color: Vec4,
    pub roughness: f32,
    pub metallic: f32,
    pub emissive: Vec3,
    pub albedo_map: Option<TextureKey>,
    pub normal_map: Option<TextureKey>,
    pub roughness_metallic_map: Option<TextureKey>,
}
```

Shader behavior per slot:
- **albedo_map**: `textureSample(...) * base_color`. Fallback: `base_color` alone.
- **normal_map**: tangent-space normal decoded via TBN matrix from vertex tangent + normal. Fallback: vertex normal.
- **roughness_metallic_map**: green channel = roughness, blue channel = metallic (glTF convention). Multiplied with scalar values. Fallback: scalar `roughness` / `metallic`.

### 3. GPU texture cache in GBufferPass

`HashMap<TextureKey, (wgpu::Texture, wgpu::TextureView)>` cache. Textures uploaded on first use. Three 1×1 fallback textures at init:
- White (1,1,1,1) for albedo
- Flat normal (0.5, 0.5, 1.0, 1.0) for normal map
- Default (1, 0.5, 0, 1) for roughness/metallic (r=unused, g=0.5 roughness, b=0 metallic)

### 4. Bind group changes

Add a third bind group (group 2) to the GBuffer pipeline:
- binding 0: albedo texture
- binding 1: normal map texture
- binding 2: roughness/metallic texture
- binding 3: sampler (shared, trilinear filtering)

Per-draw: set group 2 with the material's texture bind group. Materials without a given texture use the corresponding fallback.

### 5. Shader changes (gbuffer.wgsl)

```wgsl
@group(2) @binding(0) var t_albedo: texture_2d<f32>;
@group(2) @binding(1) var t_normal: texture_2d<f32>;
@group(2) @binding(2) var t_roughness_metallic: texture_2d<f32>;
@group(2) @binding(3) var t_sampler: sampler;
```

Fragment shader:
- Sample albedo: `textureSample(t_albedo, t_sampler, uv) * base_color`
- Sample normal map, decode from [0,1] to [-1,1], transform by TBN matrix built from vertex normal + tangent
- Sample roughness/metallic: `green * roughness_scalar`, `blue * metallic_scalar`

The vertex output already passes `world_normal`. Add `world_tangent` and `uv` passthrough for TBN construction.

### 6. GLB loader updates

Read glTF texture sources via `gltf::import` (images already decoded):
- `pbr_metallic_roughness().base_color_texture()` → albedo_map
- `normal_texture()` → normal_map
- `pbr_metallic_roughness().metallic_roughness_texture()` → roughness_metallic_map

Convert images to RGBA8, add to World, store keys in Material.

### 7. Shadow pass

Unchanged — depth-only, no texture binding needed.

## Acceptance Criteria

- [ ] `TextureData` + `TextureKey` in World
- [ ] `Material` gains `albedo_map`, `normal_map`, `roughness_metallic_map`
- [ ] GBufferPass uploads textures on first use, caches GPU handles
- [ ] GBuffer shader samples all three texture types with correct fallbacks
- [ ] Normal mapping via TBN matrix from vertex tangent + normal
- [ ] GLB loader reads all three texture types from glTF
- [ ] Sketchfab model renders with correct colors and surface detail
- [ ] Existing procedural cubes still render correctly (scalar materials unchanged)
- [ ] All tests pass, clippy clean
