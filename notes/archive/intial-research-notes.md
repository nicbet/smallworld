## Pointers

- Sparse Voxel Directed Acyclic Graphs (SVDAGs) or multi-level sparse voxel octrees (SVOs) to aggressively compress data
- Volumetric Level of Detail (LOD): They dynamically stream and downsample data based on the player's distance, maintaining strict memory footprints (often under 4 GB) over vast horizons
- Modern Raytracing & Shading: Engines utilize custom WebGPU or compute shaders to calculate real-time global illumination, propagation volumes, and screen-space refractions across millions of voxels simultaneously
- Real-time Destructible Terrain: True volumetric data tracks the interior of objects, allowing seamless digging, erosion, and cave carving without regenerating complex polygon meshes
- GPU Fluid Dynamics: Engines often simulate procedural water flow, basin accumulation, and physics by tracking cellular occupancy across voxel grids
- Volumetric Environmental Sims: Modern iterations feature complex grid-based heat calculations, allowing dynamic weather, indoor warmth retention, and survival mechanics to naturally adapt to custom-shaped rooms

## Architecture

The fundamental architecture of a microvoxel engine vs. a traditional (Minecraft) voxel engine is completely different:

- Data Structure: Minecraft engines use flat 3D arrays of chunks (16 × 16 × 256). Microvoxel engines require hierarchical trees, like Sparse Voxel Directed Acyclic Graphs (SVDAGs), to compress billions of sub-centimeter points.
- Rendering: Minecraft engines use polygonal meshing (Greedy Meshing) to turn voxel faces into triangles for the GPU. Microvoxel engines bypass triangles entirely, using GPU Raymarching to trace pixels directly through the data tree.

To transition, you will need to replace your chunk-generation and meshing scripts with a GPU-driven raymarcher, keeping Godot mainly as your editor, input, and physics coordinator.

### SVDAG vs SVO

An SVDAG (Sparse Voxel Directed Acyclic Graph) is significantly more performant and memory-efficient than a standard SVO (Sparse Voxel Octree).

#### Advantages

- Massive Memory Reduction: SVDAGs typically compress geometry data by 70% to 90% compared to an SVO. This allows you to fit highly detailed microvoxel worlds directly into high-speed VRAM.
- Higher L1/L2 Cache Efficiency: Because the data structure is radically smaller, more of the voxel data stays inside the GPU's closest hardware caches during raymarching. This reduces costly trips to system memory.

#### Disadvantages

The primary downside of an SVDAG is that it is much harder to modify in real-time. When a player destroys a single microvoxel, an SVO can simply traverse the tree and change one leaf node. An SVDAG cannot do this easily because that node might be shared by thousands of other objects in the world. Editing requires a complex "un-merging" process or a secondary modification buffer on the CPU

### Comparison of Engines

| Feature / Metric            | Godot 4 (GDExtension + Compute)                                                                    | Unity (DOTS / Custom SRP)                                                                                 | Unreal Engine 5 (Nanite / RDG)                                                                      |
| --------------------------- | -------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Data Structure Architecture | ❌ Manual. You must build SVDAG logic and memory layouts entirely from scratch in C++.             | 🟡 Semi-Manual. Custom structures required, but DOTS provides native cache-friendly layout tools.         | Built-In. Nanite uses a highly optimized, production-ready micro-geometry DAG structure.            |
| CPU-Side Acceleration       | 🟡 Good. GDExtension allows raw C++ execution, but lacks an automated SIMD compiler framework.     | Elite. Burst Compiler and Jobs System automatically vectorize CPU code for SVDAG generation.              | Excellent. Massive, highly multi-threaded C++ engine architecture built for heavy asset processing. |
| VRAM & Disk Streaming       | ❌ None. You must manually code the multi-threaded asynchronous disk-to-GPU paging framework.      | 🟡 Partial. Addressables and native graphics buffers help, but streaming logic must be custom-built.      | Native. Virtual Asset streaming pages micro-geometry into VRAM seamlessly based on player view.     |
| Shadows & Lighting          | ❌ Incompatible. Default shadow maps fail at micro-scale; requires writing custom GPU ray-tracers. | 🟡 Customizable. Scriptable Render Pipeline (SRP) makes it easy to hook custom ray-tracers into lighting. | Out-of-the-Box. Virtual Shadow Maps (VSM) and Lumen natively handle high-density micro-geometry.    |
| Development Velocity        | 🟡 Slow Start. High initial setup overhead to get basic voxel raymarching working on the GPU.      | 🟡 Moderate. Fast CPU iteration via C# DOTS, but graphics pipeline setup takes significant time.          | Fast Prototyping. Can utilize or reverse-engineer existing Nanite source code immediately.          |
| Licensing & Overhead        | Perfect. MIT License. Zero royalties, lightweight engine size, and completely open source.         | 🟡 Fair. Fee per seat/install thresholds; moderate engine bloat and complex package management.           | ❌ Heavy. 5% royalty after $1M revenue. Massive engine footprint and strict hardware requirements.  |

## Notes

Traditional games swap an entire model when it crosses a distance boundary (LODs). A microvoxel engine should stream and merge nodes dynamically based on Screen-Space Error (SSE). If you are using an SVO or SVDAG structure, you naturally get layers of detail because every parent node represents the average data of its eight children.

Design your data structure so that a parent node is rendered whenever its children would occupy less than 1 to 2 pixels on the player's screen. For a millimeter-to-centimeter scale microvoxel world spanning a few kilometers, you typically want a tree depth of 14 to 16 levels. The engine seamlessly traverses deeper into the tree for objects close to the camera, and stops early at level 4 or 5 for objects on the far horizon.

Unreal Engine 5’s Nanite is a massive breakthrough because it achieves constant-time rendering. Whether a scene has 1 million triangles or 10 billion, the performance cost remains mostly flat. It achieves this through several core engineering tricks:

1. Fixed 128-Triangle Clusters (The "Voxel" Analogy)
   Nanite does not process full 3D models. During compilation, it breaks meshes into micro-chunks called clusters, each containing exactly 128 triangles. This fixed size is crucial because 128 triangles fit perfectly into the parallel processing architecture of modern GPU compute units.

2. The Directed Acyclic Graph (DAG) Hierarchy
   Nanite groups adjacent 128-triangle clusters together and simplifies them into a new parent cluster with half the triangle density. This forms a Directed Acyclic Graph (DAG).
   The Secret: The boundaries between clusters are locked during simplification. This ensures that when the engine renders a high-detail cluster right next to a low-detail cluster, the edges line up perfectly with no visible gaps or cracking.

3. Two-Pass Culling on the GPU
   Instead of using the CPU to figure out what is visible, Nanite pushes all culling to the GPU using a highly optimized two-pass pipeline:
   Pass 1: The GPU checks the previous frame’s depth buffer. If a cluster was hidden behind a wall last frame, it is instantly culled (Occlusion Culling).
   Pass 2: For everything else, the GPU calculates exactly how big the cluster is on screen. If a cluster's triangles are smaller than a single pixel, the engine automatically swaps it for a lower-resolution parent cluster from the DAG.

4. Bypassing the Hardware Rasterizer (The Software Path)
   Standard GPUs are designed to render triangles using hardware rasterizers, but they slow down significantly when triangles become smaller than a single pixel (a problem known as quad-overdraw).
   The Secret: If a triangle is larger than a few pixels, Nanite uses the GPU's standard hardware pipeline. However, if the triangles are microscopic (pixel-sized), Nanite bypasses the graphics hardware entirely and uses a custom, hyper-fast software rasterizer written inside a Compute Shader.

Rendering high-end geometry is pointless if you cannot light it. Lumen is Epic's real-time Global Illumination (GI) and reflection engine. It solves the immense performance cost of ray-tracing through a smart hybrid framework:

1. The Surface Cache
   Tracing rays against billions of Nanite micro-triangles in real-time is impossible for consumer hardware. To bypass this, Lumen looks at the world from multiple angles and flattens the surrounding geometry into low-resolution, 2D texture cards called the Surface Cache. When a light ray bounces and hits a wall out of your view, Lumen does not evaluate complex materials; it simply looks up the color value on that 2D texture card.

2. Screen-Space Tracing First
   When calculating reflections or light bounces, Lumen always starts with Screen-Space Ray Tracing (SSRT). It traces rays directly across the pixels already rendered on your screen. This capture method is incredibly fast and highly detailed, but it can only track objects that are currently visible to the player.

3. World-Space SDF Tracing Fallback
   If a ray travels off the screen (for example, hitting a wall behind the player), Lumen transitions to tracing through a Signed Distance Field (SDF) volume. An SDF is essentially a low-resolution, blurry 3D cloud representation of your game world. Tracing rays through a simplified 3D cloud requires very little processing power, allowing Lumen to track global bounces and reflections across massive environments in real time.

That check is damning, and it reframes the whole thread. Let me go through your four points against what's actually in the code.

1. Locked boundaries — we have nothing
   No skirts, no boundary locking, no stitching. And chunk_mesher.cpp:864 does this:

neighbor_solid = false; // boundary: always emit
Every downsampled chunk emits its full boundary shell regardless of what's next door. So adjacent LOD levels don't crack — they overlap, with back-to-back coplanar faces buried in the terrain. Cracks are hidden by accident, paid for in overdraw.

This is the single most important thing on your list, because crack-free transitions are what permit aggressive LOD. Nanite can drop detail the instant it goes sub-pixel only because the DAG guarantees the seam is watertight. We can't drop detail aggressively because we have no such guarantee — so we compensate by holding full resolution far too long. The voxel version doesn't need a DAG: skirts are the cheap fix, boundary-locked downsampling the principled one.

1. The software rasterizer — the lesson isn't the rasterizer
   Their real defence against sub-pixel triangles is the LOD selection in point 3. The software path is the fallback for when it's unavoidable. And here's the arithmetic for us, at 637 px and 75° FOV — focal length 415 px, so a 12.5 cm voxel subtends 51.9 / d pixels:

distance voxel size
13 m 4.0 px
26 m 2.0 px
52 m 1.0 px
70 m 0.74 px
The LOD 1 band runs 13→70 m and holds 69% of all our geometry. Most of it is at or below one pixel, where a hardware rasterizer shades a 2×2 quad minimum — roughly 14% efficiency at 0.74 px.

That explains the measurement that didn't make sense earlier: 3.41M quads ran at 66 FPS while 5.87M ran at 65. We are not triangle-throughput bound. We are bound by how many sub-pixel quads we submit.

And my fog floor is what forces it. Holding the first geometric step until 70 m guarantees a band of sub-pixel geometry. I built the exact pathology Nanite exists to avoid, then told you to buy it with view distance. That was wrong twice over.

What I'd actually adopt, ranked
Never submit sub-pixel geometry. Delete the fog floor, let SSE drive, target quads ≥ ~2 px. This is the dominant cost and it's a policy change, not a rewrite.
Crack-free transitions (your point 2) — skirts first, boundary-locking if skirts show. This is the prerequisite that makes 1 safe.
Finer LOD granularity (your point 1) — mixed-LOD superchunk assembly; a 16 m unit picking one level is far too coarse.
Occlusion culling (your point 3, pass 1) — Godot has CPU occlusion culling. For a voxel world, especially underground, the hidden fraction is large and nobody has measured it.
Not adoptable in Godot's scene renderer: the compute software rasterizer and GPU indirect draw. Those need the render loop we don't own.

On Lumen — Godot's SDFGI is the same family (world-space SDF tracing with screen-space first). Worth revisiting, but geometry is the live problem.

One thing worth stating plainly: Nanite renders triangles. It's the strongest existing evidence that meshed geometry scales to absurd density — which is an argument for fixing our LOD ladder, not for abandoning meshes. The parts of it that are genuinely better than 20-year-old Minecraft tech are the error metric, the watertight hierarchy, and the culling. All three are portable.
