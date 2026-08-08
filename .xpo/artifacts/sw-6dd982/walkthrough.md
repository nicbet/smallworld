## What was built

Three new GBuffer channels that downstream passes will consume: screen-space velocity for TAA/motion blur, a material ID for debug viz and material overrides, and a source flag to distinguish raymarched from rasterized pixels.

## GBuffer layout after this change

| Slot | Format | Channel | Budget |
|---|---|---|---|
| depth | Depth32Float | depth | 4 B/px |
| @location(0) | Rgba8UnormSrgb | albedo | 4 B/px |
| @location(1) | Rgba8Unorm | normal (octahedral .xy, .zw reserved) | 4 B/px |
| @location(2) | Rgba8Unorm | material (roughness .x, metallic .y, .zw reserved for AO/SSS/aniso/specular) | 4 B/px |
| @location(3) | Rgba16Float | emissive | 8 B/px |
| @location(4) | Rg16Float | **velocity** (NDC delta) | 4 B/px |
| @location(5) | R16Uint | **aux** (bits 0-14 = material ID, bit 15 = source flag) | 2 B/px |
| **Total** | | | **30 B/px** |

## How the pieces fit together

### Velocity channel

**FrameUniforms** (`gbuffer.rs`, `gbuffer.wgsl`) gained `prev_view_proj`. The GBuffer pass tracks the previous frame's view-projection matrix via `prev_view_proj: Option<Mat4>` on `GBufferPass`. First frame uses the current matrix (zero velocity); subsequent frames use the actual previous.

**DrawUniforms** gained `prev_model: mat4x4<f32>`. Static meshes write `prev_model = model`, giving camera-only velocity. When animation or physics arrives, callers write the actual previous transform for per-object velocity.

The **vertex shader** computes both current and previous clip-space positions:
```
out.cur_pos_clip = frame.view_proj * world_pos;
out.prev_pos_clip = frame.prev_view_proj * (draw.prev_model * position);
```

The **fragment shader** divides by w and writes the NDC delta:
```
out.velocity = cur_ndc - prev_ndc;
```

### Aux channel (material ID + source flag)

**DrawUniforms** gained `material_id: u32` (replaces part of the old `_pad` field — struct stays at a clean 16-byte-aligned size, now 176 bytes with `prev_model`).

The fragment shader writes `draw.material_id & 0x7FFFu` — bit 15 is always 0 (rasterized). The composite pass (future) will OR in `1u << 15u` for raymarched pixels.

Read-side unpacking for future consumers:
```
let material_id = aux & 0x7FFFu;       // 32,767 max
let is_raymarched = (aux >> 15u) & 1u;  // 0 = raster, 1 = raymarch
```

### Shade pass integration

`shade.wgsl` declares `gbuf_velocity` (binding 6, `texture_2d<f32>`) and `gbuf_aux` (binding 7, `texture_2d<u32>`). The Rust-side bind group layout and bind group in `LightingPass` include both. The shader doesn't read them yet — this establishes the contract.

### Shadow pass

`shadow.wgsl` and `lighting.rs`'s `DrawUniforms` were updated to match the new layout (the shadow shader only reads `model`, but the struct must stay in sync for consistent dynamic uniform stride).

## Key decisions

- **`R16Uint` over `Rg16Uint` for aux** — 2 bytes/pixel saves ~16 MB/frame bandwidth at 4K. Bit ops (`& 0x7FFF`, `>> 15`) are single-cycle. 15-bit material ID (32K) is sufficient; upgradeable to `R32Uint` if needed.
- **`prev_model` stubbed now** — avoids a future uniform layout change that would cascade to WGSL struct, draw_stride, and every upload site. Cost: 64 bytes/draw × 256 max = 16 KB.
- **Material `.zw` reserved** — earmarked for AO, subsurface scattering, anisotropy, and specular tint.
