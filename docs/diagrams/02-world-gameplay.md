# World & Gameplay — ECS, Components, Events, Behaviors

The Game Object Model (ECS, per the two-questions split), the core component set, the
double-buffered event bus, and the three-backend behavior model. See: Composability &
Scripting, Describing a Game, OQ 6/20.

@import "02-world.mmd" {as="mermaid"}

## Core Components (plain data)

@import "02-components.mmd" {as="mermaid"}

## Behavior Model (one contract, three backends)

@import "02-behavior.mmd" {as="mermaid"}

Mutation semantics (all backends): spawn/add/remove **immediate**; despawn **deferred to end
of frame**; behaviors spawned this frame start next frame.
