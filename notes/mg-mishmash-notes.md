# MGMishMash Voxel Engine Dev Notes

```
Currently opted for meshing rather than DDA/Raytracing, partially because the GPU i'm on has no hardware support, and while DDA was faster for terrain in isolation, given the over-saturation of fragment hardware and under-utilization of vertex hardware, the net performance for the workload in practise was worse.

I'll likely explore having an RT path if the GPU has RT hardware, at least for the static data like terrain, far-away trees etc;

As far as specifics, nothing too fancy, the usual bits for relative chunk position, material etc; I'm opting not to use normals as both a stylistic choice and to save on data, so that's an easy win for lighting (I guess even though the normal face direction is fairly cheap, but still).

I also like Meshing for props as I quite like using vertex animation. This is used for the tree sway + leaf animations and for grass stomping mechanics (which I have actually found to be quite necessary from a gameplay standpoint to avoid ugly visual clipping between creatures and plants).
```

---

```
Thanks, yeah the render distance is pretty low, have been looking into some optimizations like Macro chunks, but the main challenge I have been facing atm is having the far LOD look good enough that it doesn't break the feel. Conversely, right now immersion feels worse with a greater viewing distance because the Far LOD is simultaneously low-detail and visually noisy, which is very distracting.

I will likely look into some sort of DoF or haze, but the actual cost is manageable.

I'm also running on a fairly mid-range GPU (Apple M1 Pro), so better GPUs could likely handle the current detail band at a higher fidelity out of the box.
```

---

```
Firstly, geometry shaders are horrifically inefficient as they do not behave nicely with GPU hardware, compute shaders are much better for generation. You can always upper bound the geometry allocation threshold for generation and compact later. Even modern mesh shaders have their flaws.

As much as raytracing and raymarching have their place, especially for insanely high level of details, GPUs are also just incredibly optimized and fast at geometry processing these days.

However, reducing the work is still important. Frustum culling chunks, occlusion culling, LODs etc; all of the old fashioned tricks still get you pretty far if you’re going for geometry
```

---

```
Thank you! Lighting is quite simple, I have a coarse grid that runs a cellular automata which does one iteration of light propagation per cell per frame, so it’s pretty cheap, and bounded to the volumes around lights. (Around 0.5% of the frame cost).

The torch propagates around 3 coarse cells in each direction. Supporting intensity and color propagation. I have since enhanced it from this video to have some material effects which are samples during propagation.

The second feature is what I call “Occupancy AO”, probably already a concept, but I calculate the % amount a coarse cell is filled with voxels. This value is used to weight the rate of propagation, and is also used to apply additional AO darkening so that crevices are naturally darkened.
```

---

```
fully rasterized, I found raymarching to perform significantly worse at this scale or require some serious upsampling that just ruined the visuals, I'm just not very good with the ray tracing though so I'd imagine I did something wrong, but for a like for like scene; using my 3070 I get ~1800 FPS here, with a ray traced version of the same scene it was closer to ~200. Which sounds great but when you switch to an igpu that's more like ~50 without any postprocessing, lighting, sky, entities, just terrain so i abandoned that route entirely.

eah tbh I had a similar experience with RT/RM, in some cases faster, but limiting and hard to get right.

I guess RT typically has a higher base fixed cost, but should be fairly stable no matter the contents.

Although raster gives you so much flexibility for dynamic effects, and having a live depth buffer makes other things more efficient, plus vertex hardware is just insanely fast 😅

Curious how many verts are in your scene? Are you using greedy meshing etc? I use it for static props but not for chunks, as it’s expensive to run dynamically.

I’m definitely in need of some additional optimizations, but all in good time :)

Tbh the bottleneck is cpu gen at the moment rather than pure gpu perf, which becomes an issue at further viewing distances
```

---

```
There is no “correct” way to generate terrain and any considerations depend on what you want to achieve. The GPU is very good at highly parallel tasks, but not all aspects of terrain gen fall into this category. For a simple noise sampling for a heightmap, great, but some aspects such as feature generation, carving etc can be more bespoke, and having each gpu cell evaluate may not be the fastest overall approach, depending on what you are doing.

The GPU is very good at things like marching cubes or other mesh generation, which process all cells and evaluate neighbouring ones, but operations like fills, carves, detail spawning may require more specialised routines.

Chunking can also be what you want. Fir your case, it would just mean rather than having a single large dispatch generating one mesh, you just do the same thing over smaller parts. Then you can selectively not run generation and drawing for those parts, allowing you to dial up the overall scale substantially.
```

---

```
Vertices vs. Raytracing: Yeah tbh different trade offs! Raytracing is undeniably more efficient, allows greater complexity as doubling voxel density in each dim is only one more octree layer. Raytracing also saves memory and alllows for instant updates. Cost also only scales per-pixel rather than per vertex.

However, comes with limitations in terms of animations etc; I still plan to explore hybrid, as with dedicated RT hardware (vs raymarching) the saving would be substantial for static terrain
```

---

```
Currently the engine uses vertices primarily , which means regen is slow if you have many chunk updates simultaneously (e.g snowfall). I do support a DDA-style Raymarched path for terrain and static entities, but the current implementation appear to be slower.

It seems an optimal end goal would be hybrid, ray marching for the terrain to reduce LOD requirements and enable instant manipulation, but vertex meshes for entities such as trees to enable non axis-aligned transforms and animations (trees now sway in the wind, but not in this video).

Currently, the voxel data is stored as a grid and meshed into the rendered form upon modification.

I have a DDA raymarch path for terrain, as this is non dynamic, and it is faster in isolation, but performs worse when I have dynamic content simultaneously, so preferring the traditional pipeline for the time being, especially as manipulations are typically quite localized and infrequent.

Although i’m not particularly precious about approach, the final solution sill be whatever is consistently both fast and low in memory.
```

---

```
The water uses a coarse grid which is every 1m (5x5x5 voxels), and simulation is based on using the total cell occupancy to determine how much flow is possible.

And yes, currently rasterizing triangles for the voxel terrain data, although have various data formats available as required, i.e raw voxel volume grid and some spatial acceleration structures maintained to speed up other operations.

Water is in a coarse simulation grid, converted into a mesh using a compute shader after each water tick.

The actual shading is done in a second pass, so that it can sample depth and color, but a lightweight version would be possible in the same forward pass.

The water simulation is aligned to chunks and unloaded chunks are ignored/treated as solid for simulation purposes. I maintain a simulation volume around the players actively loaded region. Water outside this is saved out to files, but only if modified from the source gen.
```

---

```
Around 10-20 million in one frame, able to get 45-85 fps on an Apple M1 Pro right now, which i think is equivalent to a GTX 1660, or Xbox series S.

Although frag shaders are not yet super optimal, so I’m confident in getting to 60 at this viewing distance.

Downside is viewing dist is fairly moderate atm, but for a minimum spec i’m fairly happy with the budget.

Fwiw the engine does already use marching cubes for LOD+2 onwards, as blocks look quite bad at distance otherwise (other videos in the playlist show this better). Doesn’t look as good as dual contouring, but computationally cheap. I currently kept the blockiness for LOD+1, as it seems to transition better - although still plenty of popping to work through!

If I switch to DDA then this could potentially be adaptive, but so far every DDA attempt has ended up slower overall. It was only faster in isolation, but the second other content used the depth buffer, the overdraw cost from lateZ loses out to meshing.
```
