# Smallworld Engine Architecture

## Overview

Smallworld is a hybrid game engine built from scratch in Rust on top of wgpu. It is a complete game engine — not a renderer, not a voxel engine. Voxel volumes and triangle meshes are both first-class geometry primitives sharing the same lighting model. The engine supports rasterization and raytracing simultaneously, with rasterization as the primary path and raytracing reserved for effects like shadows and global illumination.

The architecture takes the best ideas from Unreal Engine 5 — the Game Thread / Render Thread split, the Scene Proxy extraction model, composition-over-inheritance, data-driven design — and rebuilds them in idiomatic Rust without the C++ baggage. No `UObject` reflection system, no garbage collector, no `UPROPERTY` macros. Instead: trait objects for polymorphism, channels for cross-thread communication, SlotMap arenas for stable handles, and Rust's ownership system as the thread-safety guarantee.

### Design Principles

1. **Data-driven.** Components are plain data structs. Systems are functions that operate on component stores. No inheritance hierarchies, no virtual dispatch on hot paths.
2. **Thread ownership.** Each thread owns its data exclusively. Communication between threads happens via owned value-typed packets sent through channels — never shared mutable state. Sharing _immutable_ data across threads (`Arc` payloads, mapped staging regions) is permitted: the rule forbids shared mutability, not sharing.
3. **Game–render firewall.** Game code never sees a `wgpu::Device`, a bind group, or a GPU buffer. The extract step is the boundary. Everything above it speaks in transforms, materials, and handles. Everything below it speaks in draw commands and GPU resources. The firewall constrains _game code_, not engine internals: engine subsystems (asset pipeline, staging pool) may create and populate CPU-visible staging resources from any thread — wgpu is internally synchronized and built for it. The narrow invariant that actually matters: **the Render Thread exclusively owns device-local resources and command submission.**
4. **Handle-based resources.** Games hold opaque handles (`AssetHandle<T>`, `ResourceHandle<T>`). Lifetime, caching, and GPU upload are engine-managed. Handles are cheap to copy and safe to hold across frames.
5. **Budget-explicit.** Frame time, GPU memory, upload bandwidth, and streaming distance are explicit budgets with engine arbitration, not emergent properties.

---

@import "architecture/pipeline.md"

---

@import "architecture/render-thread.md"

---

@import "architecture/custom-render-pipeline.md"

---

@import "architecture/data-structures.md"

---

@import "architecture/composability-scripting.md"

---

@import "architecture/describing-a-game.md"

---

@import "architecture/streaming.md"

---

@import "architecture/resources-filesystem.md"

---

@import "architecture/vegetation-pcg.md"

---

@import "architecture/atmosphere-cloud-weather.md"

---

@import "architecture/physics.md"

---

@import "architecture/animation.md"

---

@import "architecture/audio.md"

---

@import "architecture/user-interfaces.md"

---

@import "architecture/lifecycle.md"

---

@import "architecture/profiling-instrumentation.md"

---

@import "architecture/frame-lifecycle.md"

---

@import "architecture/build-order.md"

---

@import "architecture/e2e-slice.md"

---

@import "architecture/capability-tiers.md"

---

@import "architecture/open-questions.md"

---
