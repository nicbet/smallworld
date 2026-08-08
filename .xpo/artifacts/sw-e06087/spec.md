## What

Add a `Capabilities` struct to `gpu.rs` that probes GPU features and limits at boot and exposes them as typed, read-only fields. Pipeline compilation and subsystem init branch on this struct instead of querying `device.features()` / `device.limits()` ad-hoc.

## Why

Build order 0 for the revised architecture. Every downstream pipeline variant (A.4) needs to know what the hardware supports — ray query, mesh shaders, HDR output, etc. Today only `TIMESTAMP_QUERY` is probed, and limits like `min_uniform_buffer_offset_alignment` are re-queried inline at every use site. A single struct built once at boot eliminates the scatter and gives pipeline compilation a clean branch point.

## Acceptance Criteria

1. `Capabilities` struct is constructed inside `GpuContext::new()` and `GpuContext::headless()`, stored as a `pub` field on `GpuContext`.
2. The struct probes and stores:
   - `timestamp_query: bool` — `TIMESTAMP_QUERY`
   - `ray_query: bool` — `EXPERIMENTAL_RAY_QUERY`
   - `mesh_shader: bool` — `EXPERIMENTAL_MESH_SHADER`
   - `shader_i16: bool` — `SHADER_I16`
   - `shader_f16: bool` — `SHADER_F16`
   - `int64_atomics: bool` — `SHADER_INT64_ATOMIC_ALL_OPS`
   - `subgroups: bool` — `SUBGROUP`
   - `hdr_color_spaces: wgpu::SurfaceColorSpaces` — queried via `surface.get_capabilities(&adapter).color_spaces(Rgba16Float)`. Stores the full bitflags; downstream code selects the actual color space.
   - `max_storage_buffer_mb: u32` — from `adapter.limits().max_storage_buffer_binding_size`
   - `min_ubo_align: u32` — from `adapter.limits().min_uniform_buffer_offset_alignment`
   - `max_texture_dim: u32` — from `adapter.limits().max_texture_dimension_2d`
   - `adapter_name: String` — from `adapter.get_info().name`
   - `backend: wgpu::Backend` — from `adapter.get_info().backend`
3. Headless path: `hdr_color_spaces` defaults to `SurfaceColorSpaces::empty()`.
4. `GpuContext::supports_timestamps()` delegates to `self.caps.timestamp_query`.
5. Existing inline `device.limits().min_uniform_buffer_offset_alignment` queries in `gbuffer.rs` and `lighting.rs` are replaced with reads from the `Capabilities` struct (passed as a value at construction time).
6. Boot log prints a capabilities summary: feature flags, key limits, HDR color spaces.
7. `Engine::caps()` accessor returns `&Capabilities`.
8. Builds and tests pass (`cargo build`, `cargo test`, `cargo clippy`).

## Flow

1. **Define `Capabilities`** in `gpu.rs`.
   - Plain struct with `pub` fields; no setters. Derive `Debug`, `Clone`.
   - `Capabilities::probe(adapter, surface)` constructor takes `&wgpu::Adapter` and `Option<(&wgpu::Surface, &wgpu::Adapter)>` for HDR probing.

2. **Expand `negotiate_features()`** to request all probed features that are available. The Capabilities struct records what's *available*; `negotiate_features` requests everything available from the probed set.

3. **HDR probing** uses `surface.get_capabilities(&adapter).color_spaces(TextureFormat::Rgba16Float)` to get `SurfaceColorSpaces` bitflags. Stored directly — no wrapper enum.

4. **Store `caps: Capabilities` on `GpuContext`**, constructed after device creation.

5. **Log capabilities summary** — one `log::info!` block.

6. **Replace ad-hoc queries:**
   - `GpuContext::supports_timestamps()` → `self.caps.timestamp_query`
   - `gbuffer.rs` UBO alignment queries → `caps.min_ubo_align` passed at construction
   - `lighting.rs` same pattern

7. **Add `Engine::caps()` → `&Capabilities`**.

8. **Verify**: `cargo build`, `cargo test`, `cargo clippy`.

## Decisions

- **Capabilities on `GpuContext`, not `Engine`** — device-level state. Subsystems that take `&GpuContext` automatically get access.
- **Flat struct, not a trait** — one GPU, one set of capabilities. Fields are `pub` and the struct is `Clone`.
- **Raw `SurfaceColorSpaces` bitflags, not a wrapper enum** — the engine's surface config code picks the actual color space later. Capabilities just records what's available for `Rgba16Float`.
- **HDR priority for logging: ExtendedSrgbLinear > Bt2100Pq > Bt2100Hlg > DisplayP3** — log the "best" available for the summary line, but store the full bitflags.
- **`max_storage_buffer_mb` as `u32`** — MB for readability. Raw byte value available via `device.limits()`.
- **Request all probed features** — no cost to enabling unused features; device is ready when pipeline code starts branching.

## Edge Cases

- **Headless: no surface** — `hdr_color_spaces` is `SurfaceColorSpaces::empty()`. All feature flags still probed from adapter. (LOW)
- **Feature requested but not granted** — `request_device()` panics on failure (unchanged). (LOW)
- **Adapter returns 0 for a limit** — stored as-is; consumers handle it. (LOW)

## Assumptions

- wgpu 30's `SurfaceCapabilities` exposes `color_spaces(TextureFormat) -> SurfaceColorSpaces` for querying supported HDR output paths. Confirmed by user.
- Pipeline compilation changes (branching on Capabilities) are out of scope — that's A.4.
