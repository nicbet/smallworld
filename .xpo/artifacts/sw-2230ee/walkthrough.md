# Walkthrough: 1 km world at 30 fps — SVO traversal optimization

## Result

1280×720 on M1 Max, 1 km × 1 km world at 10 cm voxels, shadows on:
session baseline **83.9 ms → ~35 ms** at the (worst-case) fixed bench viewpoint, and
**GPU avg 20.9 ms / p99 23.4 ms** on the 20 s flythrough — the story's AC (≥30 fps)
is met on the flythrough, which is the representative experience. User-approved
closure on those numbers; the fixed viewpoint sits 2–4 ms over and is tracked in the
60 fps follow-up.

## Where the time went (and how we knew)

The traversal was memory-latency bound: dependent random 16 B reads into a 128 MB
SVO node buffer. Every optimization here either removes dependent reads, removes
per-iteration ALU/divergence in the hot loop, or packs rays so warps stay busy.
Every change was validated with a byte-compare probe (`render_default_headless`,
`PROBE_OUT`, `ppm_diff`) against the previous state — "no visual change" was proven,
not assumed. A/B numbers were taken same-run or interleaved because absolute
medians drift ±15% with machine temperature; near the end of the session the machine
throttled so hard that shadows-off measured *slower* than shadows-on, which is why
the final AC call uses the flythrough.

## The changes, in dependency order

### 1. Single-read traversal (`raymarch.wgsl`, both `trace_svo` and `trace_svo_any`)

The old cursor-based descent re-fetched `svo_nodes[node_idx]` at classify, again in
the child-advance step, and again after every pop — ~k+2 fetches for a node with k
children. The stack frame now caches the parent's `children` base and child mask, so
each node is fetched **exactly once per ray**. `node_idx` left the frame entirely;
it exists only as a transient between descend and the child's classify.

### 2. Integer cell coords + 8-byte frames

`node_min` used to be accumulated in f32 across descents and stored per frame
(20 B × 16 depth = 320 B/thread of spill). Node position is now integer cell
coords — `cell = (cell << 1) | octant_bits` on descend, `cell >>= 1` on pop — and
`node_min = world_min + vec3<f32>(cell) * node_size` is recomputed at classify.
Key f32 fact: `(2c)·s` and `c·(2s)` round identically, so parent and child face
planes coincide *exactly* — which is also what later made the eps change safe.
Frame is 8 B (`children` + packed state); stack 128 B/thread.

### 3. Terrain-slab shadow clip (`terrain_top_y`)

Terrain bricks exist only in the bottom `grid_dims.y` brick rows of the world cube;
a shadow ray past its exit from that slab cannot hit terrain. Plumbed as
`Raymarcher::set_terrain_top_y` (called by `main.rs`, bench, probe) through the
previously dead `grid_dims` uniform slot (was hardwired `[0,0,0]` — the shader never
read it). **Trap for the future**: the first slab attempt used `u.grid_dims`
directly and silently zeroed all shadows — the bench "won" 26 ms while the probe
diff showed 4.6% of pixels changed. Bench numbers without an image compare lie.

### 4. Bit-iteration child advance

Front-to-back order visits octant `cursor ^ flip`. Since XOR by `flip` is a fixed
bit permutation, `permute_mask` (3-step butterfly) reorders the child mask once at
classify so that selection becomes `firstTrailingBit(rem)` + clear — replacing an
up-to-8-iteration scan loop with a divergent branch per slot. Byte-identical, one
of the two biggest single wins.

### 5. Absolute AABB eps

`node_size * 0.001` padding cost ~15% of the frame: at interior levels the pad is
meters wide, forcing double-descents into both children near every shared face.
Because integer cells made shared planes exact (see §2), the pad only needs to
absorb ray/AABB *arithmetic* rounding (~0.1 mm at km scale) → `VOXEL_SCALE * 0.02`
(2 mm). Image cost: 494 px (large) / 1,828 px (default) isolated single-pixel
shading flips at grazing edges; crack analysis (adjacency/run/sky-bleed) clean.

### 6. Per-brick occupancy masks (`BrickPool::write_voxels` → binding 8)

64-bit mask, one bit per 4³ chunk, computed on upload (word w maps to exactly one
chunk), gates the fine-DDA voxel fetch. Measured **~0 ms** — brick DDA is
iteration-bound, not read-bound (warp-coherent 4 KB bricks cache well). Kept
because it is near-free and enables future chunk skipping. Two shadow-side uses
were tried and discarded: chunk-granular any-hit (a surface's own crust chunk is
occupied → 47.8% of pixels self-shadowed) — unsound for surface-launched rays.

### 7. Half-res shadow pass (the 3-pass split)

**The key negative result first**: in-pass 2×2 quad sharing (workgroup memory +
barrier) saved only ~1 ms despite 4× fewer shadow rays — under SIMD lockstep every
simdgroup still stalls on its tracing lanes; decimation saves work, not wall time.
The rays must be *packed*: `cs_primary` (full res) writes a G-buffer
(world_pos+ndotl `rgba32f`, albedo `rgba8`, normal `rgba8snorm`), `cs_shadow` runs
at half res with every lane tracing, `cs_shade` composites. Shadow marginal cost:
**~21 ms → ~3.5 ms**. Separate passes are mandatory (storage-texture write and its
read can't share a usage scope); the caller's timestamp pair is split P1-begin /
P3-end so HUD "compute" still reports the total. The shadow pass is skipped
entirely when shadows are off. `ndotl` rides in `gbuf_pos.w` (−1 = miss) so
shading precision is exact f32. Quality: 0.47% of pixels (2×2 shadow-edge
quantization), user-approved from a magnified side-by-side crop.

## Follow-up (filed under epic sw-ff3ae3)

60 fps push: beam pre-pass, node re-layout — plus two loose ends from this story:
a cool-machine A/B to see whether the split costs more than its ~1 ms bandwidth
estimate at the worst-case viewpoint (candidates: pack ndotl into albedo.a for a
single 8-bit load in `cs_shade`; store depth-t `r32f` instead of `rgba32f`
world_pos), and CPU/streaming hitches (flythrough dt p99 47.5 ms is CPU-side, not
GPU).

## Related

sw-031260: `double_free_panics` gated to `debug_assertions` (found running suites
in release, the measurement profile).