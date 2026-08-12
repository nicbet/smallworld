## Audio

_(OQ 26 resolution, 2026-08-11.)_ The audio engine is **in-house** — the mixer is engine identity, not an outsourced dependency. UE5's reinvestment in its own Audio Mixer + MetaSounds is the modern precedent; Unity's built-in audio being licensed FMOD internals is the cautionary tale. Middleware (Wwise/FMOD) is deliberately not designed for: if a shipping need ever demands it, the path would be a whole-subsystem replacement plugin — noted as possible, intentionally unspec'd.

Engine/game split: the engine owns the device layer, mixer graph, voices, DSP, and streaming; the game owns _what plays and why_ — through `AudioCommands` (imperative) or the `AudioEmitter` component (declarative, engine-managed lifecycle), with game logic computing judgments like occlusion and writing engine-provided per-voice knobs. See "Who Initiates Sound" below.

### Architecture

- **Device layer: `cpal`** (the Rust ecosystem standard). The audio thread runs the mixer at a fixed block size (~512 samples), draining `AudioCommands` once per game frame.
- **Mixer graph as data.** Buses with sends and inserts, defined by a `MixerLayout` asset (the same serde philosophy as `AnimGraph`): e.g., master → music / SFX / dialogue / ambience, with per-bus volume and effect chains.
- **Voice management.** A voice pool with priorities and **virtualization**: over-limit voices keep advancing their playheads silently and resume audibly when a slot frees — 200 logical sounds stay affordable.
- **Spatialization.** Distance attenuation + stereo/surround panning in v1, driven by `set_listener` and per-voice positions. HRTF explicitly deferred.
- **DSP.** Per-voice pitch/volume/lowpass built in; one algorithmic reverb as a send-bus effect in v1; an effect-insert trait for custom DSP later.
- **Occlusion — the split, showcased.** The engine primitive is a per-voice filter/gain knob. The _game_ computes occlusion (physics raycasts through the `PhysicsProvider` query API, from a behavior or system) and writes the knob. The engine never decides what is occluded.
- **Streaming music.** Long clips decode on the IO pool into ring buffers — never fully resident. `AudioClip` remains the in-memory format for short SFX.
- **Instrumented.** The audio thread has its profiling lane; voice counts and buffer underruns join the standard counter set (OQ 24).

### Who Initiates Sound

**The engine never initiates a sound by policy** — the weather rule applied to audio: the engine makes sound _representable_; the game decides what plays. Two canonical compositions:

- **Audio triggers are not an engine concept.** A `Collider { sensor: true }` fires a trigger event into the bus → a game behavior reads it → `ctx.audio.play(suspense_track, …)`. The engine provided the sensor, the event plumbing, and the mixer; the _game_ decided the cave is scary.
- **Footsteps**: the clip's animation event track fires `footstep` into the bus (Animation section) → a game system queries the surface underfoot (physics raycast → material) → picks gravel-vs-grass → plays it. The engine fires the event; the game maps event → sound.

Two complementary game-facing surfaces, the same imperative/declarative pairing used throughout the design: **`AudioCommands`** (imperative — one-shots, music control, from `GameContext` and `BehaviorContext` alike) and the **`AudioEmitter` component** (declarative — persistent entity-attached sources with engine-managed lifecycle; see Core Engine Components).

**v2+:** MetaSounds-style procedural audio graphs — the same data-graph philosophy as `AnimGraph`, applied to DSP.
