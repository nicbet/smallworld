## What

Add a GLB/glTF loader that reads mesh geometry and PBR material properties into the World.

## Why

The engine currently requires procedural mesh construction. Real content testing (lighting, shadows, materials) needs real meshes from Blender/Sketchfab.

## How

### 1. Dependency

Add `gltf` crate to `crates/engine/Cargo.toml`. It handles binary parsing, buffer views, accessor iteration.

### 2. Loader API in `assets.rs`

```rust
pub struct LoadedScene {
    pub meshes: Vec<(Mesh, Material)>,
    pub instances: Vec<LoadedInstance>,
}

pub struct LoadedInstance {
    pub mesh_index: usize,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl LoadedScene {
    pub fn spawn(&self, world: &mut World) -> Vec<MeshInstanceKey> { ... }
}

pub fn load_glb(path: impl AsRef<Path>) -> Result<LoadedScene, String> { ... }
```

### 3. Vertex mapping

glTF accessor → our `Vertex`:
- `POSITION` → `position: [f32; 3]`
- `NORMAL` → `normal: [f32; 3]` (default `[0,1,0]` if absent)
- `TEXCOORD_0` → `uv: [f32; 2]` (default `[0,0]` if absent)
- `TANGENT` → `tangent: [f32; 4]` (default `[1,0,0,1]` if absent)

Indices: glTF u16 → u32 conversion. If no indices, generate identity indices.

### 4. Material mapping

glTF PBR metallic-roughness → our `Material`:
- `base_color_factor` → `base_color: Vec4`
- `roughness_factor` → `roughness: f32`
- `metallic_factor` → `metallic: f32`
- `emissive_factor` → `emissive: Vec3`

Texture maps ignored (scalar properties only, sw-8d894c).

### 5. Scene graph

Flatten glTF node tree. Each node with a mesh reference produces a `LoadedInstance` with the node's world transform (accumulated from parent chain). Multi-primitive meshes produce one `(Mesh, Material)` per primitive.

### 6. Sandbox update

Accept an optional CLI argument: `cargo run -- path/to/model.glb`. If provided, load it instead of the procedural test scene. Keep the lights and floor from the test scene so the model is lit and grounded.

## Acceptance Criteria

- [ ] `load_glb` reads mesh geometry and materials from a GLB file
- [ ] Vertices map correctly (position, normal, UV, tangent)
- [ ] Materials map PBR scalar properties
- [ ] Scene graph flattened with correct world transforms
- [ ] Sandbox loads a GLB from CLI arg
- [ ] At least one Sketchfab model renders correctly with existing lighting
- [ ] All existing tests pass
- [ ] `cargo clippy` clean
