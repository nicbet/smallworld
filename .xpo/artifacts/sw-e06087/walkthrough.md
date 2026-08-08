## What was built

A `Capabilities` struct in `gpu.rs` that probes GPU features, limits, and HDR surface support at engine boot. This is build order 0 — every downstream pipeline variant (A.4) will branch on this struct to select shader paths.

## How the pieces fit together

### Capabilities struct (`gpu.rs`)

`Capabilities::probe(&adapter, surface)` runs once during `GpuContext::new()` / `GpuContext::headless()`. It queries:

- **7 feature flags** from `adapter.features()`: `timestamp_query`, `ray_query`, `mesh_shader`, `shader_i16`, `shader_f16`, `int64_atomics`, `subgroups`
- **HDR color spaces** via `surface.get_capabilities(&adapter).color_spaces(Rgba16Float)` — stored as raw `SurfaceColorSpaces` bitflags so downstream surface config can pick the actual color space
- **4 limit values**: `max_buffer_mb` (max single buffer allocation), `max_ssbo_binding_mb` (max storage buffer binding per dispatch), `min_ubo_align`, `max_texture_dim`
- **Adapter identity**: `adapter_name`, `backend`

The struct is `pub` on `GpuContext`, with `Debug + Clone`. All fields are `pub` with no setters — it's read-only by convention (constructed once, never mutated).

### Feature negotiation (`negotiate_features`)

Expanded from just `TIMESTAMP_QUERY` to all 7 probed features. Returns `(Features, ExperimentalFeatures)` — the experimental token is only enabled when the adapter actually supports experimental features (ray query, mesh shaders). The `unsafe` call to `ExperimentalFeatures::enabled()` is gated with a targeted `#[allow(unsafe_code)]` since the crate denies unsafe globally.

### UBO alignment caching

`GBufferPass` and `ShadowAtlas` previously queried `device.limits().min_uniform_buffer_offset_alignment` inline — once at construction, once per render call. Both now compute `draw_stride` once from `caps.min_ubo_align` at construction and store it as a field.

### Access path

`Engine::caps()` returns `&Capabilities`, delegating to `self.gpu.caps`. Subsystems that already receive `&GpuContext` can access `gpu.caps` directly.

## Key decisions

- **Raw `SurfaceColorSpaces` bitflags, not a wrapper enum** — the engine's surface config code will select the actual color space when it configures the surface for HDR output. Capabilities just records what the hardware supports for `Rgba16Float`.
- **Two memory fields (`max_buffer_mb`, `max_ssbo_binding_mb`)** — on Metal both report ~4 GB (Metal caps individual allocations at 4 GB regardless of physical memory). On Vulkan/DX12 they diverge: `max_buffer_size` can be much larger. `max_ssbo_binding_mb` is the one brick pool and SVO sizing must respect. wgpu doesn't expose total device memory.
- **Headless path** — `hdr_color_spaces` defaults to `SurfaceColorSpaces::empty()` since there's no surface to query. All feature flags are still probed from the adapter.
