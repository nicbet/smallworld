## What

Add three new GBuffer channels — velocity, material ID, and source flag — to the rasterizer pipeline. These are "painful to add later" channels that downstream passes (TAA, motion blur, composite, debug viz) will consume.

## Why

Build order 1, blocks 4b (shadow ray offset needs source flag) and 4c (TAA needs velocity). The current GBuffer only has albedo, normal, material properties, and emissive. Without these channels, TAA cannot compute reprojection, the composite pass cannot distinguish raymarched from rasterized pixels, and debug viz cannot overlay material IDs.

## Acceptance Criteria

1. GBuffer has 6 color attachments + depth (was 4 + depth):
   - location(0): `Rgba8UnormSrgb` — albedo (unchanged)
   - location(1): `Rgba8Unorm` — normal (unchanged)
   - location(2): `Rgba8Unorm` — material (unchanged)
   - location(3): `Rgba16Float` — emissive (unchanged)
   - location(4): `Rg16Float` — **velocity** (NDC delta, xy)
   - location(5): `R16Uint` — **aux** (bits 0-14 = material ID, bit 15 = source flag)
2. Velocity includes both camera and per-object motion: `prev_view_proj` in `FrameUniforms`, `prev_model` in `DrawUniforms`. Vertex shader computes current + previous clip positions, fragment shader writes NDC delta.
3. First frame produces zero velocity (prev_view_proj initialized to current view_proj, prev_model = model).
4. Material ID is written from `DrawUniforms.material_id` (0 for now — no material registry yet).
5. Source flag is 0 (rasterized) for all rasterizer output. The composite pass (future) will write 1 for raymarched pixels.
6. `shade.wgsl` declares bindings for velocity and aux textures.
7. `LightingPass` bind group layout and bind group include the new GBuffer channels.
8. Builds and tests pass.

## Flow

### 1. GBuffer textures (`gbuffer.rs` — `GBuffer` struct)

Add two new texture+view pairs:
- `velocity_view`: `Rg16Float`, `RENDER_ATTACHMENT | TEXTURE_BINDING`
- `aux_view`: `R16Uint`, `RENDER_ATTACHMENT | TEXTURE_BINDING`

Created in `GBuffer::new()`, recreated on `resize()`.

### 2. Uniform structs (`gbuffer.rs` + `gbuffer.wgsl`)

**FrameUniforms** — add `prev_view_proj: mat4x4<f32>` (Rust: `prev_view_proj: [f32; 16]`). Grows from 64 to 128 bytes.

**DrawUniforms** — two changes:
1. Replace `_pad: [f32; 2]` with `material_id: u32, _pad: u32` (same 112-byte offset layout).
2. Add `prev_model: [f32; 16]` at the end. Grows from 112 to 176 bytes.

WGSL: `_pad: vec2<f32>` → `material_id: u32, _pad2: u32`, add `prev_model: mat4x4<f32>`.

Static meshes upload `prev_model = model` (zero object velocity). When animation/physics arrives, callers write the actual previous transform.

### 3. Shader outputs (`gbuffer.wgsl`)

**VertexOutput** — add:
- `@location(4) cur_pos_clip: vec4<f32>`
- `@location(5) prev_pos_clip: vec4<f32>`

**Vertex shader** — compute:
```
out.cur_pos_clip = frame.view_proj * world_pos;
let prev_world_pos = draw.prev_model * vec4<f32>(in.position, 1.0);
out.prev_pos_clip = frame.prev_view_proj * prev_world_pos;
```

**GBufferOutput** — add:
- `@location(4) velocity: vec2<f32>` (Rg16Float target)
- `@location(5) aux: u32` (R16Uint target)

**Fragment shader** — compute:
```
let cur_ndc = in.cur_pos_clip.xy / in.cur_pos_clip.w;
let prev_ndc = in.prev_pos_clip.xy / in.prev_pos_clip.w;
out.velocity = cur_ndc - prev_ndc;
out.aux = draw.material_id & 0x7FFFu; // bit 15 = 0 (rasterized)
```

Read-side unpacking (for future consumers):
```
let material_id = aux & 0x7FFFu;
let is_raymarched = (aux >> 15u) & 1u;
```

15 bits = 32,767 material IDs. AAA games ship ~5K materials; voxel palette engines far fewer. If we outgrow this, promote to `R32Uint` — same packing pattern, one format change.

### 4. Pipeline color targets (`gbuffer.rs`)

Extend `color_targets` array from 4 to 6:
- index 4: `Rg16Float`, no blend
- index 5: `R16Uint`, no blend

### 5. Render pass attachments (`gbuffer.rs` — `render()`)

Add 2 new color attachments:
- slot 4: `velocity_view`, clear to `(0.0, 0.0, 0.0, 0.0)` — zero velocity for sky
- slot 5: `aux_view`, clear to zero via `wgpu::Color::BLACK`

### 6. Previous view_proj tracking (`gbuffer.rs`)

Add `prev_view_proj: Option<Mat4>` field on `GBufferPass`.
- In `render()`: `let prev_vp = self.prev_view_proj.unwrap_or(view_proj);`
- After writing uniforms: `self.prev_view_proj = Some(view_proj);`

### 7. Frame uniform buffer (`gbuffer.rs`)

Resize `frame_uniform_buf` from `size_of::<FrameUniforms>()` (now 128 bytes).
Write both `view_proj` and `prev_view_proj` each frame.

### 8. Draw uniform upload (`gbuffer.rs` — `render()`)

Write `material_id: 0` and `prev_model: model` for all draws.

### 9. Shade pass bindings (`lighting.rs` + `shade.wgsl`)

**shade.wgsl** — add declarations (group 0):
```
@group(0) @binding(6) var gbuf_velocity: texture_2d<f32>;
@group(0) @binding(7) var gbuf_aux: texture_2d<u32>;
```

**lighting.rs** — extend `shade_gbuf_layout` with 2 new entries (bindings 6, 7) and extend bind group creation to include `gbuffer.velocity_view` and `gbuffer.aux_view`.

### 10. Verify

`cargo build`, `cargo test`, `cargo clippy`. Run the sandbox to verify rendering is unchanged (new channels written but not consumed by shade pass yet).

## Decisions

- **`R16Uint` for aux** — 2 bytes/pixel. Bit 15 = source flag, bits 0-14 = material ID (32,767). Saves bandwidth vs `Rg16Uint` (~16 MB/frame at 4K). Bit ops are single-cycle on all target GPUs.
- **Velocity as raw NDC delta, not UV-space** — NDC range is [-1,1] per axis. Consumers (TAA, motion blur) can scale as needed. `Rg16Float` has more than enough range and precision.
- **`prev_model` stubbed now** — avoids a uniform struct layout change later (cascades to WGSL struct, draw_stride, every upload site). Static meshes write `prev_model = model`. Cost: 64 bytes/draw × 256 max = 16 KB buffer growth. Trivial.
- **Shade bindings declared but not consumed** — the shader declares the texture variables but doesn't read them yet. This establishes the contract; consumption comes with TAA (velocity) and composite (source flag).
- **`prev_view_proj` on GBufferPass, not Engine** — it's GBuffer-specific state. The engine doesn't need to track it.

## Edge Cases

- **First frame velocity** — `prev_view_proj` is `None`, falls back to current view_proj → zero velocity. TAA needs 2+ frames to warm up anyway. (LOW — handled silently.)
- **Integer render target clear** — `R16Uint` cleared with `wgpu::Color::BLACK`. wgpu interprets color components as the target type. For integer formats, `0.0` → `0u`. (LOW.)
- **6 color attachments** — `max_color_attachments` is 8 on all target hardware (Metal, Vulkan desktop). (LOW.)

## Assumptions

- The existing material `.zw` channels remain unused (0.0). They're not repurposed — the new aux channel replaces the need.
- Raymarcher changes are deferred per the issue description ("when composite lands").
- The `material_id` field in DrawUniforms will be wired to a material registry in a future issue.

## GBuffer budget

After this change: 30 bytes/pixel (was 24, +25%). At 2560×1440: ~106 MB total. Well within the 4 GB allocation limits.
