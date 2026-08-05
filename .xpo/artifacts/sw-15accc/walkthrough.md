## What was built

Sun shadow rays and optional smooth normals from occupancy gradient, both toggleable from the debug panel. The sphere now casts a visible shadow on the ground, and the smooth normals option softens the blocky voxel silhouette.

## How the pieces fit together

### Shader restructure

`trace_brick` was restructured to return unshaded hit info (`base_color`, `normal`, `voxel`, `handle`, `world_pos`) instead of final shaded color. Shading moved to the main loop where normal selection, shadow ray, and final color computation happen in sequence.

### Shadow ray (`trace_shadow`)

A stripped-down two-level DDA that returns `bool` on first solid hit. No color, no normal tracking, no palette lookups — just voxel occupancy checks. The shadow origin is offset from the hit point by `normal * (VOXEL_SCALE * 0.5 + SHADOW_BIAS)` to clear the surface of the hit voxel (the center-of-voxel + small bias approach caused self-intersection because 0.01 < half-voxel size of 0.05).

### Smooth normals (`smooth_normal`)

Central differences of occupancy along ±x, ±y, ±z (6 voxel reads). Returns the normalized negative gradient, which points from solid toward air. Out-of-brick positions are treated as air to avoid cross-brick lookups. Falls back to the face normal if the gradient is zero.

### Uniform flags

`_pad1` in the uniform struct was repurposed as `flags: u32`. Bit 0 = shadows, bit 1 = smooth normals. The viewer sets these from debug panel checkboxes. The shader reads them with bitwise AND and branches — the GPU handles uniform-driven branches efficiently since all threads in a workgroup take the same path.

### Camera position

Initial camera moved from (0, 2, 5) to (0, 10, 18) with -25° pitch so the full scene (sphere, ground, shadow) is visible on startup.

## Key decisions

- **Shadow origin = voxel center + normal × (half_voxel + epsilon)**, not voxel center + small epsilon. The original 0.01 bias was inside the 0.1m voxel, causing every shadow ray to self-intersect.

- **Separate `trace_shadow` function** rather than reusing `trace_brick` with a flag. The shadow ray needs no color, palette, or normal — stripping those makes it faster per ray.

- **Frame Time window starts collapsed** to keep the viewport unobstructed.

## What a future reader should know

- Smooth normals only sample within the current brick (out-of-brick = air). This produces slightly wrong normals at brick edges, but those pixels already have correct face normals from the coarse DDA entry normal.

- The shadow ray has no max distance — it traces until it exits the world or hits something. For large worlds, a distance cutoff would improve performance.

- The `flags` field in the uniform is extensible — bits 2+ are available for future toggles.
