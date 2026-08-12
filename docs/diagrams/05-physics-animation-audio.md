# Simulation Subsystems — Physics, Animation, Audio

See: Physics, Animation, Audio sections; OQ 6/14/16/25/26/28/30.

## Physics — Provider Model

@import "05-physics.mmd" {as="mermaid"}

Steps exclusively in `fixed_update`; sync-back through `get_mut` (change tracking fires);
events into the double-buffered bus; sim state rebuilds from component descriptions on load.

## Animation — Pose Primitives + Data Graphs

@import "05-animation.mmd" {as="mermaid"}

Sampling/graph evaluation on the game worker pool in PostUpdate; the palette is the sole seam
to the GPU DeformPass. Events fire into the bus; root motion is delivered, never auto-applied.

## Audio — In-House Mixer

@import "05-audio.mmd" {as="mermaid"}
