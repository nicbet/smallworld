
# SVO Node Structure + GPU Buffer — Walkthrough

## What was built

A Sparse Voxel Octree (`svo.rs`) that replaces `BrickIndex` as the engine's spatial index. Each node stores averaged child colors for seamless LOD — any node in the tree can be rendered as a single colored voxel at the appropriate size.

## Node format

`SvoNode` is 16 bytes, `#[repr(C)]` + `Pod`/`Zeroable` for direct GPU upload:
- `children: u32` — index where 8 children start (0 = leaf/empty)
- `brick: u32` — BrickPool handle at leaves (`u32::MAX` = none)
- `color: u32` — packed RGBA, averaged from descendants
- `flags: u32` — bits 0-7: valid child mask, bit 8: has brick data

Children are allocated as contiguous blocks of 8, so child i is at `nodes[children + i]`. This makes octant addressing a single addition.

## Color propagation

`update_colors()` walks the tree recursively. At leaves, the color is the brick's average color (passed during `insert_brick`). At interior nodes, the color is the occupancy-weighted average of all non-empty children's colors. This means every node in the tree carries a correct color representation of its entire subtree — the shader can stop traversal at any level and shade from that color.

## Memory

At 16 bytes/node with contiguous 8-child blocks: a 1km world at 1.6m leaf resolution needs ~10 levels of octree. Sparse terrain (60% solid) produces ~5-20M nodes = 80-320 MB. Well within the M1 Max's 4 GB buffer limit.
