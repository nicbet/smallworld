## What

Sun shadow ray per hit and optional smoothed normals from occupancy gradient, both toggleable from the debug panel.

## Why

Face normals from DDA give the classic voxel look but no self-shadowing — the entire sunlit side of the sphere is flat-lit. A shadow ray through the same traversal adds depth. Smoothed normals soften the blocky silhouette where desired.

## Acceptance Criteria

- Solid voxels cast shadows via a sun-direction ray through the two-level DDA
- Shadow toggle in the debug panel (default on)
- Optional smooth normals via 6-neighbor occupancy gradient (default off)
- Smooth normal toggle in the debug panel
- `cargo clippy` and `cargo test` pass

## Flow

### 1. Shader changes — `raymarch.wgsl`

**Restructure `trace_brick`:** Return unshaded hit info (`base_color`, `normal`, `world_pos`) instead of final shaded color. Shading moves to the main loop.

**New: `trace_shadow(ro, rd) -> bool`:** Stripped-down two-level DDA — no color, no normal, no palette lookup. Returns `true` on first solid voxel hit. Early-exits aggressively.

**New: `smooth_normal(handle, voxel) -> vec3<f32>`:** Central differences of occupancy along ±x, ±y, ±z (6 reads). Out-of-brick positions treated as air. Returns normalized negative gradient; falls back to face normal if gradient is zero.

**Uniform change:** Repurpose `_pad1` → `flags: u32`. Bit 0 = shadows enabled, bit 1 = smooth normals enabled.

**Main loop shading:**
```
normal = select face or smooth
ndotl = max(dot(normal, SUN_DIR), 0)
shadow = 1.0 if shadows off, else trace_shadow(hit_pos + normal*eps, SUN_DIR)
color = base * (AMBIENT + (1-AMBIENT) * ndotl * shadow)
```

### 2. Rust uniform — `raymarcher.rs`

Rename `_pad1` → `flags` in `Uniforms`. `render()` sets flags from passed booleans.

### 3. Viewer — `main.rs`

Add `shadows: bool` (default true) and `smooth_normals: bool` (default false) to `RunState`. Checkboxes in the debug panel. Pass to `render()`.

## Decisions

**D1: Shadow ray is a separate function, not reusing `trace_brick` in a different mode.** The shadow ray needs no color, no palette, no normal — stripping those makes it ~2x faster per ray. Duplicating the DDA structure is acceptable for a hot-path function.

**D2: Smooth normals use 6 neighbors (central differences), not 26 (full 3×3×3).** 6 reads vs 26. The gradient from central differences is smooth enough; the full cube only helps at corners.

**D3: Out-of-brick voxels treated as air for smooth normals.** Avoids cross-brick reads. Produces slightly wrong normals at brick boundaries, but those pixels are already correctly shaded by the coarse normal.
