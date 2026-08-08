# Clustered Shading Depth Distributions

Research for sw-dcc28a (shadow atlas + light grid + deferred shade).

## Logarithmic (= Exponential)

Used by Doom 2016 (id Tech 6, 16x8x24 grid) and Olsson et al. 2012. "Exponential" and "logarithmic" refer to the same scheme.

```
Z_k = near * (far / near) ^ (k / N)
k = floor(N * log(z / near) / log(far / near))
```

**Strengths:** Self-similar clusters across depth range. O(1) slice lookup. Matches perspective compression so light density per cluster stays uniform.

**Weaknesses:** Breaks when `near` approaches 0 (`log(0)` undefined). With very small near planes (0.01), first slices become extremely thin. With `near=0.1, far=1000` (10000:1 ratio), first slice spans only 0.1 to 0.12.

## Hybrid (Linear + Logarithmic)

Discussed by Tiago Sousa / CryEngine and Persson/Olsson at SIGGRAPH 2013:

```
Z_k = lerp(near + (far - near) * (k / N),
           near * (far / near) ^ (k / N),
           lambda)
```

`lambda` controls blend (0 = linear, 1 = logarithmic). Typical: 0.8-0.95.

Solves near-plane singularity by spreading near slices more uniformly. Handles `near -> 0` gracefully.

## Engine Usage

| Engine | Distribution | Grid Size |
|--------|-------------|-----------|
| Doom 2016 | Logarithmic | 16x8x24 |
| Unity URP Forward+ | Logarithmic | 32 depth slices |
| UE5 | Logarithmic | (clustered forward, being deprecated) |
| Godot 4 Forward+ | Logarithmic | Standard practice |

## Decision

Logarithmic distribution with clamped near-plane: `cluster_near = max(camera_near, 0.5)`. This avoids the singularity without hybrid complexity. If extreme depth ratios cause issues later, adding the hybrid `lerp` with `lambda ~0.9` is a one-line change.

Grid: Doom-style 16×9×24 (80px tiles at 1280×720, 24 log depth slices).

## Sources

- [Olsson et al. 2012 — Clustered Deferred and Forward Shading](https://www.cse.chalmers.se/~uffe/clustered_shading_preprint.pdf)
- [Practical Clustered Shading — Emil Persson, Avalanche Studios](https://www.humus.name/Articles/PracticalClusteredShading.pdf)
- [DOOM 2016 Graphics Study — Adrian Courreges](https://www.adriancourreges.com/blog/2016/09/09/doom-2016-graphics-study/)
- [A Primer On Efficient Rendering Algorithms — Angel Ortiz](http://www.aortiz.me/2018/12/21/CG.html)
