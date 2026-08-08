## What Was Built

GLB/glTF mesh import — the engine's first file-based asset loader. Reads mesh geometry, PBR materials, and scene graph from binary glTF files.

## How It Works

### Loader (`assets.rs`)

`load_glb(path)` uses the `gltf` crate to parse GLB files. For each mesh primitive:

1. Reads vertex attributes via `primitive.reader()` — POSITION, NORMAL, TEXCOORD_0, TANGENT. Missing attributes get sensible defaults (up normal, zero UV, X tangent).
2. Reads indices (u16→u32 conversion handled by `into_u32()`). Non-indexed meshes get identity indices.
3. Extracts PBR material scalars: `base_color_factor`, `roughness_factor`, `metallic_factor`, `emissive_factor`.
4. Reads `material.double_sided()` for backface culling control.

The scene graph is flattened by recursively walking glTF nodes, accumulating transforms via `parent_transform * local_transform`. Each node with a mesh produces `LoadedInstance`s with decomposed position/rotation/scale.

`LoadedScene::spawn(world)` adds all meshes, materials, and instances to the World in one call.

### Double-Sided Support

glTF materials can declare `"doubleSided": true` — common for foliage, cloth, and thin geometry. Implementation:

- `MeshInstance.double_sided: bool` — flows from glTF into the World
- `GBufferPass` creates two render pipelines: one with `cull_mode: Some(Back)`, one with `cull_mode: None`
- Draw loop selects pipeline per draw call based on the flag
- `gbuffer.wgsl` uses `@builtin(front_facing)` to flip the normal for back-faces, so lighting is correct from both sides

### Sandbox Integration

`cargo run -- path/to/model.glb` loads the model instead of the procedural test scene. Lights are added automatically. Falls back to the cube scene on load error.

## Key Decisions

**`double_sided` on MeshInstance, not Material.** In the engine's data model, materials are shared — multiple instances can reference one material. Double-sided is a per-instance rendering property in our pipeline (it controls pipeline selection, not material parameters).

**No texture support.** Scalar material properties only. Models that rely on painted textures show white baseColor (the default `[1,1,1,1]` multiplier). Texture map support is sw-8d894c, moved back to E1.5 since it's essential for real content.

**Dependency: `gltf` crate v1.** MIT licensed, mature, handles all binary parsing, buffer views, and accessor type conversion. Pulls in `image` for embedded textures (unused by us currently but needed by the crate).
