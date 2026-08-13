# Preface {.unnumbered}

This book documents the architecture, design decisions, and implementation details of the Smallworld engine — a hybrid game engine built from scratch in Rust on top of wgpu. Voxel volumes and triangle meshes are both first-class geometry primitives sharing the same lighting model. The engine supports rasterization and raytracing simultaneously, with rasterization as the primary path and raytracing reserved for effects like shadows and global illumination.

The architecture takes the best ideas from Unreal Engine 5 — the Game Thread / Render Thread split, the Scene Proxy extraction model, composition-over-inheritance, data-driven design — and rebuilds them in idiomatic Rust without the C++ baggage. No `UObject` reflection system, no garbage collector, no `UPROPERTY` macros. Instead: trait objects for polymorphism, channels for cross-thread communication, SlotMap arenas for stable handles, and Rust's ownership system as the thread-safety guarantee.
