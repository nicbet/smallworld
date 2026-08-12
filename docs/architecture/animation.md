## Animation

_(OQ 25 resolution, 2026-08-11.)_ The pose-computation half; the GPU half is the Deformation stage (OQ 14), and the seam between them is exactly one artifact: the bone palette. Engine/game split: the engine owns assets, sampling, blending primitives, palette production, events, and sockets; the game owns deciding _what plays_ — parameters, logic, state.

### Layered Architecture: Pose Primitives + Data Graphs

Two public levels, deliberately — UE and Unity are both secretly this shape (AnimBP compiles to AnimNodes; Mecanim sits on Playables), but they retrofitted the layering; we design it:

1. **Pose primitives (engine core, public).** Sample a clip at a time → pose; blend, additive, and masked-combine poses; IK solver nodes (two-bone, look-at; full-body deferred). Procedural animation and custom rigs build directly on this level.
2. **`AnimGraph` assets (engine-provided standard).** Blend trees, layers, masks, and state machines as serde data — RON-editable, per the data-driven design section — evaluated by the engine on top of the primitives. Games drive graph _parameters_ from behaviors (`speed`, `grounded`, `aiming`); the graph decides poses.

### Assets

- **`SkeletonAsset`** — joint hierarchy, bind pose, socket definitions. Imported from glTF.
- **`AnimClipAsset`** — joint curves + a named event track. **Compressed at cook time** (keyframe reduction + quantization, ACL-inspired; the codec is an implementation decision under OQ 27's pipeline). Clips are among the largest asset classes; compression is not optional.
- **`AnimGraphAsset`** — the data-driven graph: blend trees, layers, masks, state machines, parameter declarations.

### The Animator Component

```rust
struct Animator {
    graph:      AssetHandle<AnimGraphAsset>,
    parameters: AnimParams,                  // name → f32 / bool / trigger — plain data
}
```

Sampling and graph evaluation run on the **game worker pool in PostUpdate**, producing bone palettes that ride the staging pool into the `DeformPass`. Plain data throughout — the `Animator` serializes (OQ 20); pose state is transient and rebuilds on load.

### Events, Root Motion, Sockets

- **Animation events.** Clips carry named event tracks; sampling fires them into the double-buffered event bus — footsteps ride the same machinery as everything else.
- **Root motion.** Extracted by the engine from the root track and _delivered_ — a per-frame component field the game reads in Update — never auto-applied. The game routes it through the character controller or transform itself; UE and Unity both converged here after years of fighting the alternative.
- **Sockets.** Bone-level attachment extends the existing hierarchy: `world.attach_to_bone(child, parent, joint)`. `WorldTransform` propagation consumes the sampled pose, so weapon-in-hand rides the same parent mechanism as everything else.
