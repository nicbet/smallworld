# Grid-shaped chasms: f32 octant misclassification analysis

Standalone reproduction of the insertion descent used by `Svo::insert_brick`
before the fix, with the Default preset's exact arithmetic
(`world_size = (32*16)*0.1`, `world_min = -(32*1.6)/2`, `BRICK_SIZE = 16*0.1`).

Brick min corners sit mathematically *on* octree split planes. The old code
classified them with `pos >= center` where `center` accumulated through f32
halving, while `pos` came from `world_min + g * BRICK_SIZE`. The two rounding
paths disagree by up to ~4e-7 (the 1.6 representation error amplified by `g`),
flipping the comparison at specific planes.

## Program

```rust
fn insert_leaf_index(pos: f32, world_min: f32, world_size: f32, leaf: f32) -> (u32, f32) {
    let mut node_min = world_min;
    let mut node_size = world_size;
    let mut idx = 0u32;
    while node_size > leaf * 1.01 {
        let half = node_size * 0.5;
        let center = node_min + half;
        idx *= 2;
        if pos >= center {
            idx += 1;
            node_min += half;
        }
        node_size = half;
    }
    (idx, node_min)
}
```

## Output (32-cell x/z axis, 12-cell y axis)

```
BRICK_SIZE = 1.60000002384185791
world_size = 51.20000076293945312
world_min  = -25.60000038146972656
--- x/z axis (32 cells) ---
brick 7:  pos=-14.40000057220458984 landed in cell 6
brick 14: pos=-3.20000076293945312  landed in cell 13
brick 15: pos=-1.60000038146972656  landed in cell 14
brick 19: pos=4.79999923706054688   landed in cell 18
brick 20: pos=6.39999961853027344   landed in cell 19
brick 23: pos=11.19999885559082031  landed in cell 22
brick 25: pos=14.39999961853027344  landed in cell 24
brick 28: pos=19.19999885559082031  landed in cell 27
brick 30: pos=22.39999961853027344  landed in cell 29
--- y axis (12 cells) ---
brick y=5: landed in cell 4
brick y=7: landed in cell 6
```

Every misplaced brick overwrote its lower neighbor's leaf and left its own
cell empty: one-brick-wide missing planes at those indices, two-brick-wide
where indices are adjacent (14+15, 19+20) — the visible "grid shaped chasms".

## Fix

`insert_brick` now snaps the position to the leaf lattice once
(f64 multiply + round, robust to ±half a cell of error) and descends by
integer bit tests — no float comparisons anywhere in the descent, so
placement is exact at any tree depth. Regression test
`min_corner_inserts_land_in_distinct_leaves` inserts the full 32×12×32 grid
with the preset's exact arithmetic and asserts one distinct brick leaf per
cell (old code collapses ~2.6k of them).
