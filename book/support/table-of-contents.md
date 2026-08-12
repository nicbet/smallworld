Organizing this massive document into a cohesive Table of Contents requires shifting the focus from a flat list of engine features to a pedagogical progression. Jason Gregory’s *Game Engine Architecture* is the perfect template because it builds from the ground up: establishing the OS/memory boundaries first, building the world representation next, and only then hooking up the heavy simulation and rendering systems.

However, since this engine is uniquely data-driven, built in Rust on `wgpu`, and treats voxel volumes as first-class citizens alongside standard meshes, we need to adapt Gregory's structure heavily. We also need to add a few crucial missing pieces that every complete engine book requires—specifically, foundational mathematics, networking, and a deeper dive into tooling.

Here is a proposed textbook Table of Contents, tailored to the specific architecture of this engine.

---

# Modern Game Engine Design

### Part I: Foundations & Engine Support Systems

*Gregory starts with C++ and memory. Here, we start with Rust idioms, thread boundaries, and memory budgets.*

* **Chapter 1: Architectural Foundations**
* 1.1 The Game-Render Firewall: Decoupling Simulation from Presentation


* 1.2 Data-Driven Design vs. Object-Oriented Hierarchies


* 1.3 Thread Ownership and the Work-Stealing Pools




* **Chapter 2: The Core Loop and Frame Lifecycle**
* 2.1 The Two-Frame Overlapped Pipeline


* 2.2 Frame Pacing, Vsync, and Dynamic Resolution


* 2.3 Fixed-Timestep Simulation and Interpolation


* 2.4 Engine Lifecycle: Resizing, Device Loss, and Graceful Teardown




* **Chapter 3: Memory, Handlers, and Instrumentation**
* 3.1 Generational Indices and `SlotMap` Arenas


* 3.2 Explicit Budgets: The `BudgetRegistry`

* 3.3 Telemetry: Unified CPU/GPU Profiling with Tracy




* **Chapter 4: The Resource Pipeline and Virtual Filesystem**
* 4.1 Asset Identity: GUIDs vs. Aliases


* 4.2 The Cooking Pipeline: Derived-Data Caches


* 4.3 The Staging Pool: Asynchronous GPU Uploads





### Part II: Gameplay Foundations (The World)

*Before rendering the world, we must represent it. This part covers the ECS, scripting, and spatial scale.*

* **Chapter 5: The Entity-Component-System (ECS)**
* 5.1 Plain Data Components and Dense Storage


* 5.2 Describing the World: Asset Descriptors and Spawning


* 5.3 Change Tracking: The Engine's Delta Generator




* **Chapter 6: Game Logic, Scripting, and Events**
* 6.1 The `BehaviorHost`: Abstracting Rust, C, and Lua


* 6.2 Why Code and Text Data Outperform Visual Node Graphs
* 6.3 The Double-Buffered Event Bus


* 6.4 Decoupled Communication via Gameplay Tags




* **Chapter 7: Managing Massive Scale**
* 7.1 The Precision Problem: Why Float32 Fails
* 7.2 Large-World Coordinates (LWC) in Float64


* 7.3 Cell-Anchored Rendering Offsets




* **Chapter 8: World Streaming and Data Residency**
* 8.1 Layer 1: Spatial Partitioning and Entity Streaming


* 8.2 Layer 2: Detail Streaming and the Demand/Fulfill Pipeline


* 8.3 State Persistence: The Base-and-Overlay Model





### Part III: Rendering (The Presentation)

*Now we cross the firewall. This part leans into the `wgpu` backend, the render graph, and hybrid geometry.*

* **Chapter 9: The Render-Thread Firewall**
* 9.1 The Extract Phase and the `FramePacket`

* 9.2 The Retained Scene and GPU Resource Pools


* 9.3 Feedback: Reading GPU Timestamps and Cull Stats




* **Chapter 10: The Render Graph and Pipeline Extensibility**
* 10.1 Directed Acyclic Graphs (DAGs) for Pass Management


* 10.2 Custom Geometry Extractor and Renderer Traits


* 10.3 Hardware Capability Gating (e.g., Apple Silicon M-series vs. Desktop GPUs)




* **Chapter 11: Visibility and Geometry**
* 11.1 CPU vs. GPU Culling (Frustum and HZB Occlusion)


* 11.2 The Shared Mesh Stream and Dithered LOD Fades


* 11.3 Voxel Volumes as First-Class Geometry (Raymarching Proxies)


* 11.4 Vertex Deformation and Compute Skinning




* **Chapter 12: Shading, Lighting, and Global Illumination**
* 12.1 The Deferred GBuffer Layout and Shading Models


* 12.2 Clustered Light Assignment and Shadow Atlases


* 12.3 The Indirect Ladder: Software GI Clipmaps to Hardware Ray Tracing




* **Chapter 13: Environment and Post-Processing**
* 13.1 Atmosphere LUTs, Clouds, and Weather States


* 13.2 Volumetrics and Froxel Media


* 13.3 Vegetation & Procedural Content Generation (PCG)


* 13.4 The Post-Process Chain: TAAU, Auto-Exposure, and Tone Mapping





### Part IV: Simulation, Audio, and Networking

*Fleshing out the remaining simulation components. Networking is currently missing from the design doc, but is mandatory for a complete textbook.*

* **Chapter 14: Physics and Collision**
* 14.1 The Physics Provider Model (Agnostic Interfaces for Rapier/Jolt)


* 14.2 Kinematic Character Controllers vs. Dynamic Bodies


* 14.3 Joints, Constraints, and Raycast Queries




* **Chapter 15: Animation Systems**
* 15.1 Pose Primitives and Blending


* 15.2 Data-Driven `AnimGraph` Evaluation


* 15.3 Root Motion and Bone Sockets




* **Chapter 16: Audio Architecture**
* 16.1 The Mixer Graph and DSP Effects


* 16.2 Voice Virtualization and Spatialization


* 16.3 Imperative Commands vs. Declarative Emitters




* **Chapter 17: Networking and Multiplayer *(New Chapter)***
* 17.1 Client-Server Architecture and Determinism Requirements
* 17.2 State Replication and Interpolation
* 17.3 Rollback Netcode for Fixed-Tick Simulation



### Part V: Tools & User Interface

*Closing the book with the developer experience and player-facing interfaces.*

* **Chapter 18: User Interface and Tooling**
* 18.1 Dev-UI: Immediate Mode Integration (egui)


* 18.2 Game-UI: Widgets as ECS Entities and Flexbox Layouts


* 18.3 Device-Agnostic Action Mapping


* 18.4 Building the Editor as an Engine Application





---

### Why this structure works

This structure immediately identifies the "missing" areas of your architecture document.

1. **Networking:** While the document mentions fixed-tick simulation and hints at deterministic guarantees to keep "rollback netcode viable", a complete engine textbook needs a dedicated chapter detailing how states are replicated across the wire.


2. **3D Math Foundations:** Gregory dedicates massive page counts to SIMD math, Quaternions, and matrix transformations. While implied by your `Transform` components, standard textbook practice would likely require an early math primer, which I wove into the LWC and ECS chapters to keep it practical.


3. **Visual Scripting Alternatives:** By dedicating Chapter 6 to explaining *why* text-based data logic is superior, the book can actively dismantle the industry's reliance on visual node graphs and champion cleaner coding architectures.
