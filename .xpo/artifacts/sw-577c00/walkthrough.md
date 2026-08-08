## What was built

The GBuffer pass — first sub-stages of Execute. Renders visible cached meshes into a GBuffer (albedo, normal, material) with depth, then blits albedo to the swapchain as a debug view. Replaces the placeholder cube renderer. Data flows end-to-end for the first time: World → Cull → Stream → GBuffer → screen.

## How the pieces fit together

### Shaders

**`gbuffer.wgsl`** — vertex shader transforms mesh vertices by model + view_proj matrices. Fragment shader writes three MRT outputs:
- Location 0: albedo (Rgba8UnormSrgb) — material `base_color`
- Location 1: normal (Rgba8Unorm) — octahedral-encoded world-space normal
- Location 2: material (Rgba8Unorm) — roughness in R, metallic in G

Octahedral normal encoding maps a unit sphere to [0,1]² for Unorm storage. Standard technique (< 1° max error), used by UE5. `sign_not_zero()` + `oct_encode()` in the fragment shader, `oct_decode()` documented for the future lighting shader.

**`hzb.wgsl`** — compute shader for HZB mip chain downsampling (max of 2x2 blocks). Written and registered but the build step is deferred (see "What to know for future work").

### `gbuffer.rs`

**`GBuffer`** — owns depth (Depth32Float) + 3 color textures (albedo, normal, material) + their views. Created at surface dimensions, recreated on resize. Total: 16 bytes/pixel at 1080p ≈ 33 MB.

**`GBufferPass`** — owns:
- GBuffer render pipeline (MRT with 3 color attachments + depth)
- Frame uniform buffer (view_proj matrix, per-frame)
- Draw uniform buffer (model matrix + material properties, dynamic offsets, 256 draws max)
- Debug blit pipeline (reuses `blit.wgsl` fullscreen triangle, samples albedo)
- HZB builder (texture allocated, build deferred)

Per-draw uniforms use dynamic uniform buffer offsets aligned to `min_uniform_buffer_offset_alignment`. Each draw writes model matrix + base_color + roughness/metallic at `draw_index * aligned_stride`.

**`render()` flow:**
1. Write frame uniforms (view_proj)
2. Collect draws from `StreamOutput` — volume meshes (identity model, grey albedo) and mesh instances (computed model matrix, material from World)
3. GBuffer render pass: for each draw, set dynamic offset + vertex/index buffers, draw indexed
4. HZB: ensure texture allocated
5. Debug blit: sample albedo → swapchain surface

### Engine integration

`PlaceholderRenderer` replaced by `GBufferPass`. `render_frame` now runs the full pipeline: `drain_changes` → `cull` → `stream` → `gbuffer_pass.render(stream_output)` → present. The borrow checker handles field splitting correctly — `stream_output` borrows `self.stream_stage` while `gbuffer_pass.render` borrows `self.gbuffer_pass` and `self.gpu`.

### Sandbox fix

Floor quad indices changed from `[0, 1, 2, 0, 2, 3]` to `[0, 2, 1, 0, 3, 2]` — the original winding produced a downward geometric normal, causing backface culling to hide the floor when viewed from above. Vertex normal attribute `(0, 1, 0)` doesn't affect GPU face culling.

## Key decisions

- **No explicit position buffer** — reconstructed from depth + inverse VP in the future lighting shader. Saves 33 MB of Rgba32Float texture.
- **Octahedral normals in Rgba8Unorm** — 2 channels, < 1° error, half the bandwidth of Rgba16Float.
- **Dynamic uniform buffer offsets** — one large buffer pre-filled with all draws' data, dynamic offset per `set_bind_group` call. Avoids per-draw buffer creation.
- **Debug blit to albedo** — validates pipeline end-to-end without lighting. Removed when sw-dcc28a (lighting) takes over.
- **HZB build deferred** — Depth32Float cannot be reinterpreted as R32Float in wgpu. Needs a separate compute pass with `texture_depth_2d` binding. Filed as sw-76487b.

## What to know for future work

- **HZB mip chain build** not wired up (sw-76487b). Texture allocated, shader written, compute dispatch deferred. CullStage still passes `None` for HZB.
- **Normal transform** uses `(model * vec4(normal, 0.0)).xyz` + normalize. Correct for uniform scale and rotation-only transforms. Non-uniform scale would need inverse-transpose — file follow-up if MeshInstance.scale becomes non-uniform in practice.
- **Per-draw uniform writes** cap at 256 draws. Instanced/indirect drawing tracked in sw-117099.
- The blit creates a new bind group every frame (albedo texture view). Could cache this, but it's trivially cheap.
