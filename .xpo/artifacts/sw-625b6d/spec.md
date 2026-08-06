
# Direct Coarse Worldgen

## What

A GPU compute shader that generates coarse mip data (levels 2–4) directly by evaluating noise at sub-block centers — without first generating full 16³ voxels. Replaces the current `populate_coarse_mips()` path which generates 4096 voxels per brick then averages down.

## Why

Current pipeline: generate 16³ (4096 samples) → compute mips → extract levels 2–4.
New pipeline: generate levels 2–4 directly (1–64 samples per brick).

For 150K non-air bricks:
- Current: 150K × 4096 = 614M noise evaluations + CPU mip computation (~2.2s total)
- New: 150K × 73 = 11M noise evaluations (~50ms estimated)

This makes km-scale worlds feasible. A 1km world (25M bricks) at level 4 only: 25M noise evaluations — sub-second GPU compute.

## How

### New shader: `crates/sandbox/shaders/worldgen_coarse.wgsl`

Same noise functions as `worldgen.wgsl`. New entry point that generates levels 2–4 directly.

For each mip sub-block, evaluate the density function at the sub-block center. If density > 0, determine material color. Pack as RGBA u32 (same format as existing mip data). Occupancy alpha derived from density magnitude.

```wgsl
@compute @workgroup_size(73, 1, 1)
fn cs_generate_coarse(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let brick_idx = wg_id.x;
    let grid_pos = requests[brick_idx].xyz;
    let brick_min = params.world_min + vec3<f32>(grid_pos) * params.brick_size;

    // li maps to one of 73 mip entries (64 level-2 + 8 level-3 + 1 level-4)
    // Compute the sub-block center and evaluate noise there
    let sub_center = mip_entry_center(li, brick_min, params.brick_size);
    let density = sample_density(sub_center);
    let material = sample_material_from_density(sub_center, density);
    let packed = pack_mip_entry(material, density);
    
    output[brick_idx * COARSE_MIP_WORDS + li] = packed;
}
```

73 threads per workgroup = one thread per mip entry. Each thread:
1. Computes its sub-block center position from the mip entry index
2. Evaluates density + material at that position (1 noise call)
3. Packs RGBA and writes to output

### Helper: `mip_entry_center(li, brick_min, brick_size)`

Maps a linear mip entry index (0–72) to its world-space center:
- li 0..63 → level 2 (4³): sub-block size = brick_size/4
- li 64..71 → level 3 (2³): sub-block size = brick_size/2
- li 72 → level 4 (1³): sub-block center = brick center

### Material from density

Instead of the full sample_material() which evaluates separate cave noise, use a simplified version for coarse levels:
- density > 0 and near surface (density < 0.08) → grass color
- density 0.08–0.25 → dirt
- density > 0.25 → stone
- density ≤ 0 and below water level → water
- Caves skipped at coarse level (they're sub-brick detail)

Alpha = `clamp(density * 8.0, 0.0, 1.0) * 255` for solid, 0 for air.

### Rust side: `GpuWorldGenerator`

Add `generate_coarse()` method alongside existing `generate_all()`:

```rust
pub fn generate_coarse(
    &mut self,
    dims: [u32; 3],
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    coarse: &mut CoarseMipGrid,
)
```

Uses the coarse shader pipeline. Output buffer: `BATCH_SIZE × 73 u32` per batch.
Reads back and writes directly to `CoarseMipGrid` — no `BrickData`, no `HashMap` cache.

### Integration in `scenes.rs`

Replace the current two-step process:
```rust
// OLD: generate full 16³ for everything, then extract mips
gpu_gen.generate_all(dims, device, queue);        // 1.6s
gpu_gen.populate_coarse_mips(coarse, queue);       // 590ms
```

With:
```rust
// NEW: generate coarse mips directly for the whole world
gpu_gen.generate_coarse(dims, device, queue, coarse);  // ~50ms estimated

// Full 16³ gen only runs via the pager for nearby bricks
```

The `generate_all()` call is removed from preset setup. The pager's workers handle full 16³ generation for nearby bricks on demand (via `GpuCachedSource` as before).

## Decisions

1. **73 threads per workgroup** — one thread per mip entry. Simple mapping, no cross-thread coordination.
2. **Skip caves at coarse level** — cave noise (2 extra fbm3d calls) is sub-brick detail. Not visible at level 2+. Saves 60% of noise evaluations.
3. **Density-based alpha** — instead of counting solid/air children (which requires the full data), derive occupancy from the density function value. Approximate but visually correct at distance.
4. **Separate shader file** — keeps the full-res worldgen shader clean. Shared noise functions could be factored into a common include, but WGSL has no includes — duplication is acceptable for now.

## Acceptance criteria

- [ ] `worldgen_coarse.wgsl` generates levels 2–4 directly from noise
- [ ] `GpuWorldGenerator::generate_coarse()` dispatches coarse shader and writes to `CoarseMipGrid`
- [ ] Large World preset uses coarse gen instead of full gen + mip extraction
- [ ] Coarse gen for 262K cells completes in < 200ms (vs current ~2.2s)
- [ ] Distant terrain visually matches current output (same colors, similar shapes)
- [ ] Full 16³ gen still works for nearby bricks via pager
