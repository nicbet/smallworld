## Describing a Game

At its core, describing a game in smallworld is about implementing the `App` trait, populating a `World` with entities and components, and letting the engine handle the rest.

### 1. The Core Hierarchy (Entities & Components)

Every game object is an entity — an opaque ID — with components attached. Components are plain data. There is no base class and no required components; an entity with just a `Transform` and a `LightSource` is a light, an entity with a `Transform` and a `MeshRenderer` is a renderable object, and an entity with just a `GameplayTagContainer` is a logical marker.

#### Entity

```rust
// EntityId is a generational index — stable across insert/remove cycles
struct EntityId {
    index:      u32,
    generation: u32,
}

// EntityFlags control engine behavior
bitflags! {
    struct EntityFlags: u8 {
        const ACTIVE   = 0x01;  // participates in update
        const VISIBLE  = 0x02;  // participates in rendering — toggling emits draw upserts/removes through the delta stream
        const STATIC   = 0x04;  // hint: never moves (enables caching)
    }
}
```

#### Core Engine Components

These are the components the engine defines and understands. Games can define additional components.

##### Transform & Hierarchy

```rust
struct Transform {
    position: DVec3,   // f64 — large-world coordinates (OQ 30)
    rotation: Quat,
    scale:    Vec3,
}

// Computed by the engine's TransformSystem — game code reads but never writes
struct WorldTransform {
    matrix:      Mat4,
    inverse:     Mat4,
    prev_matrix: Mat4,   // previous frame, for velocity / motion vectors
}
```

Entities can form parent-child hierarchies. The engine propagates local transforms through the hierarchy to produce `WorldTransform` components automatically.

```rust
impl World {
    fn set_parent(&mut self, child: EntityId, parent: Option<EntityId>);
    fn children(&self, parent: EntityId) -> &[EntityId];
    fn parent(&self, child: EntityId) -> Option<EntityId>;
}
```

##### Fixed-Timestep Interpolation

_(OQ 6 resolution, 2026-08-11.)_ Entities driven by fixed-tick simulation (physics bodies) store their previous fixed-tick transform; the extract step samples `lerp(prev_tick, curr_tick, alpha)` at the fixed-step accumulator's blend factor. Smooth motion at any refresh rate, for ≤ 1 fixed tick of visual latency. Extrapolation is rejected — it predicts through collisions and pops.

- **Two distinct "previous transforms" exist and must not be conflated.** The previous _fixed-tick_ transform (interpolation input, stored per simulated entity) is not `WorldTransform.prev_matrix` (the previous _rendered frame's_ matrix — the motion-vector input, derived at extract). They differ whenever render rate ≠ tick rate.
- **Only fixed-tick-driven entities interpolate.** `update()`-driven entities — notably the camera — pass through directly, so look input pays zero added latency.
- **Teleports snap.** The teleport API sets prev = curr, so a teleport never smears across a tick.

##### Camera

```rust
struct Camera {
    projection: Projection,
    target:     RenderTargetRef,   // Screen, or an offscreen texture (RTT, probes)
    priority:   i32,               // ordering among active cameras (split-screen, PiP)
    active:     bool,
    exposure:   Exposure,          // per-camera exposure (OQ 12)
}

enum Projection {
    Perspective  { fov_y: f32, near: f32, far: f32 },
    Orthographic { height: f32, near: f32, far: f32 },
}

enum RenderTargetRef { Screen, Texture(ResourceHandle<RenderTexture>) }

enum Exposure {
    Auto(AutoExposureParams),   // histogram metering — see Post-Processing
    Manual { ev: f32 },         // cinematic / artistic control
}
```

A camera is an entity with a `Transform` and a `Camera` component — view matrices derive from its `WorldTransform`. Every active camera becomes a `ViewParams` entry in the `FramePacket`; multiple active cameras give split-screen and render-to-texture without special cases.

##### Rendering

```rust
struct MeshRenderer {
    mesh:            AssetHandle<MeshAsset>,
    material:        ResourceHandle<MaterialDef>,
    cast_shadows:    bool,
    receive_shadows: bool,
    double_sided:    bool,
    lod_bias:        f32,          // multiplier on LOD distance thresholds
    render_layer:    RenderLayer,  // bitmask for camera filtering
}

struct VolumeRenderer {
    source:          VolumeSourceId,      // plain-data handle — generator registered with the Voxel Plugin
    source_params:   VolumeSourceParams,  // per-entity generator inputs (seed, offset, …) — plain data
    bounds:          AABB,
    lod_policy:      LodPolicy,
    stream_priority: StreamPriority,
}

// Generators register once with the Voxel Plugin under a stable name; entities reference them
// by handle — the same discipline as assets and behaviors. Saves reference sources by name,
// exactly as they reference assets by path (OQ 20). One generator serves any number of entities.
// (OQ 21, 2026-08-11: `source` was previously a `Box<dyn VolumeSource>` inside the component —
// moved behind a handle to restore the plain-data component rule.)
impl VoxelPlugin {
    fn register_source(&mut self, name: &str, source: impl VolumeSource + 'static) -> VolumeSourceId;
}

trait VolumeSource: Send + Sync {
    fn generate(&self, params: &VolumeSourceParams, coord: BrickCoord, world_min: Vec3) -> Option<BrickData>;
    fn bounds(&self, params: &VolumeSourceParams) -> AABB;
    fn lod_hint(&self) -> LodMeta;
}
```

##### Lighting

```rust
struct LightSource {
    kind:        LightKind,
    color:       Vec3,
    intensity:   f32,
    cast_shadow: bool,
    shadow_bias: f32,
}

enum LightKind {
    Directional { direction: Vec3, cascade_count: u8 },
    Point       { radius: f32, falloff: Falloff },
    Spot        { direction: Vec3, radius: f32, inner_angle: f32, outer_angle: f32, falloff: Falloff },
}

enum Falloff { InverseSquare, Linear }
```

##### Fog & Media

Local participating media is an entity with a `Transform` and a `FogVolume` component, injected into the froxel grid each frame. Global height fog lives in `EnvironmentParams` (OQ 11).

```rust
struct FogVolume {
    shape:      FogShape,   // Box | Sphere — local bounds derive from Transform
    density:    f32,
    albedo:     Vec3,
    emission:   Vec3,
    anisotropy: f32,        // Henyey-Greenstein g, −1..1
}
```

##### Physics

Plain-data _descriptions_ — the physics provider owns simulation state internally, linked by handle and rebuilt from these on load (see the Physics section).

```rust
struct RigidBody {
    body_type: BodyType,   // Dynamic | Kinematic | Static
    mass:      f32,
    drag:      f32,
    ccd:       bool,       // continuous collision detection for fast movers
}

struct Collider {
    shape:       ColliderShape,   // Sphere | Capsule | Box | ConvexHull | TriMesh
    friction:    f32,
    restitution: f32,
    layers:      CollisionLayers, // bitmask: collision filtering
    sensor:      bool,            // trigger volume — events only, no collision response
}
```

##### Audio

`AudioListener` marks the ears (typically the camera or player head; exactly one active).
`AudioEmitter` is the **declarative complement** to the imperative `AudioCommands`: persistent, entity-attached sound whose _mechanical lifecycle_ the engine manages — starts/stops with the entity and its range, position follows `WorldTransform`, virtualized by distance through the voice pool — while _what_ it emits stays game-declared data. Commands remain the tool for one-shots and music control; emitters for stateful sources (campfire, machinery, ambience). Serializes like any component (OQ 20): a saved campfire keeps crackling.

```rust
struct AudioListener;

struct AudioEmitter {
    clip:     AssetHandle<AudioClip>,
    volume:   f32,
    pitch:    f32,
    spatial:  bool,
    looping:  bool,
    range:    f32,     // audibility / virtualization radius
    autoplay: bool,    // start when spawned or entering range
    bus:      BusId,   // MixerLayout routing
}
```

##### Reflection Probes (spec'd, deferred)

Not v1. Trigger: **authored** interior content (buildings, ships). Probes are deliberately _not_ the answer for procedural voxel interiors — a static capture goes stale on destruction; the SVO sky-visibility and voxel-traced specular slots cover those. Captured via aux views, prefiltered by the Environment/IBL machinery, assigned per cluster like lights, sampled with parallax box projection.

```rust
struct ReflectionProbe {
    shape:          ProbeShape,   // Box | Sphere — extents from Transform
    blend_distance: f32,
    resolution:     u32,          // cubemap face size
    update:         ProbeUpdate,  // Static (capture once) | Dynamic { interval_frames: u32 }
}
```

##### Materials

Shared via `ResourceHandle` — multiple entities can reference the same material. Mutable at runtime.

```rust
struct MaterialDef {
    base_color:             Vec4,
    roughness:              f32,
    metallic:               f32,
    emissive:               Vec3,
    emissive_intensity:     f32,
    albedo_map:             Option<AssetHandle<TextureAsset>>,
    normal_map:             Option<AssetHandle<TextureAsset>>,
    roughness_metallic_map: Option<AssetHandle<TextureAsset>>,
    emissive_map:           Option<AssetHandle<TextureAsset>>,
    alpha_mode:             AlphaMode,
    double_sided:           bool,
}

enum AlphaMode { Opaque, Mask(f32), Blend }
```

#### Custom Game Components

Games define their own components as plain Rust structs. The only requirement is `Send + Sync + 'static`.

```rust
// Game-defined component — the engine doesn't know about it
struct Health {
    current: f32,
    max:     f32,
}

struct Inventory {
    slots: Vec<Option<ItemId>>,
    capacity: usize,
}

// Attach to entities just like engine components
world.add(player, Health { current: 100.0, max: 100.0 });
world.add(player, Inventory { slots: vec![None; 20], capacity: 20 });
```

### 2. The Gameplay Framework

Unlike UE5, smallworld does not impose a rigid GameMode/GameState/Controller hierarchy. Instead, it provides building blocks that games compose as needed.

#### The App Trait

The game's entry point. The engine calls these methods at defined points in the frame.

```rust
trait App {
    fn init(&mut self, ctx: &mut GameContext);
    fn update(&mut self, ctx: &mut GameContext, dt: f32);
    fn fixed_update(&mut self, ctx: &mut GameContext, fixed_dt: f32);
    fn shutdown(&mut self);
}
```

- `init` — called once after the engine initializes the World. Load assets, spawn initial entities, set up game state.
- `update` — called once per frame with variable delta time. Process input, run gameplay logic, animate.
- `fixed_update` — called at a fixed rate (default 60 Hz, configurable). Physics integration, network tick, anything that needs deterministic timestep. May run 0–N times per frame depending on accumulated time; runs **zero ticks while `Time.paused`**, and the accumulator advances by _scaled_ time, so `Time.scale` is slow-motion with per-tick determinism intact (OQ 33). **Engine guarantee (OQ 19):** no engine system introduces nondeterminism into fixed-tick simulation — this keeps lockstep/rollback netcode viable when networking arrives.
- `shutdown` — called once before exit. Save state, clean up.

#### GameContext

Everything the game needs to interact with the engine, bundled into a single borrow.

```rust
struct GameContext<'a> {
    world:  &'a mut World,
    input:  &'a Input,
    time:   &'a Time,
    assets: &'a mut AssetServer,
    audio:  &'a mut AudioCommands,
    events: &'a mut EventBus,
    window: &'a WindowState,
}

// Registration APIs and render feedback are methods on GameContext,
// backed by engine state outside the public fields:
impl GameContext<'_> {
    // Init-time registration
    fn register_geometry_backend(&mut self, extractor: impl GeometryExtractor + 'static,
                                 renderer: impl GeometryRenderer + 'static);
    fn register_system(&mut self, phase: Phase, system: impl System + 'static);
    fn set_draw_processor(&mut self, pass: &str, processor: impl DrawProcessor + 'static);

    // Game flow (OQ 33)
    fn begin_world_load(&mut self, scene: WorldDescriptor) -> WorldLoadHandle;
    fn world_load_progress(&self, handle: WorldLoadHandle) -> LoadProgress;
    fn swap_world(&mut self, handle: WorldLoadHandle, transition: SwapTransition);
    // SwapTransition::None | CaptureLastFrame — freeze-frame crossfade texture (OQ 33)
    fn set_paused(&mut self, paused: bool);
    fn set_time_scale(&mut self, scale: f32);

    // Runtime settings (OQ 33) — EngineConfig holds INITIAL values only
    fn set_pacing(&mut self, pacing: PacingConfig);
    fn set_window_mode(&mut self, mode: WindowMode);

    // Render feedback — fn feedback(), fn gpu_frame_time(): defined in
    // Frame Pipeline — Render-to-Game Feedback (not repeated here).
}

struct Time {
    dt:       f32,    // SCALED delta — 0 while paused; game logic reads this
    real_dt:  f32,    // unscaled wall-clock delta — UI/menus read this (OQ 33)
    elapsed:  f64,    // total scaled seconds since start
    frame:    u64,    // frame counter
    fixed_dt: f32,    // fixed timestep (e.g. 1/60)
    scale:    f32,    // 1.0 normal, 0.5 slow-mo — drives the fixed accumulator too (OQ 33)
    paused:   bool,   // fixed accumulator frozen; update continues (OQ 33)
}

struct WindowState {
    width:        u32,
    height:       u32,
    scale_factor: f64,
    focused:      bool,
    mode:         WindowMode,
}
```

#### Engine Entry Point

```rust
struct EngineConfig {
    title:           String,
    window_mode:     WindowMode,
    fixed_timestep:  f32,        // default 1/60
    pipeline_mode:   PipelineMode,
    pacing:          PacingConfig,   // vsync, target frame time, DRS, latency mode (OQ 8)
    render_budget:   RenderBudget,
    log_level:       LogLevel,
}

impl Engine {
    fn run(config: EngineConfig, app: impl App + 'static) -> !;
}
```

The engine creates the World internally and hands it to `App::init()` via `GameContext`. This ensures internal component stores and change tracking are configured before the game touches anything.

#### Systems

Games can register systems — functions that run each frame over component data — for logic that doesn't belong in the monolithic `App::update()`.

```rust
trait System: Send {
    fn name(&self) -> &str;
    fn run(&mut self, world: &mut World, dt: f32);
}

impl GameContext<'_> {
    fn register_system(&mut self, phase: Phase, system: impl System + 'static);
}

enum Phase {
    PreUpdate,     // before App::update — engine systems (input, time)
    Update,        // during App::update — game systems
    PostUpdate,    // after App::update — physics, animation
    LateUpdate,    // after PostUpdate — hierarchy propagation, bounds recomputation
}
```

Engine-internal systems (transform propagation, streaming demand, change tracking) run in `LateUpdate` and are not user-visible.

### 3. World Building

#### Scenes & Levels

A `World` contains all entities for the current level. Level transitions swap the entire World. For streaming open worlds, the engine supports region-based loading — entities within a geographic region are spawned and despawned based on camera distance (see the Streaming section for the full two-layer design).

```rust
struct LoadedScene {
    meshes:    Vec<(String, MeshAsset)>,
    materials: Vec<(String, MaterialDef)>,
    textures:  Vec<(String, TextureAsset)>,
    instances: Vec<SceneInstance>,
    lights:    Vec<SceneLight>,
}

struct SceneInstance {
    name:      String,
    mesh:      usize,       // index into meshes
    material:  usize,       // index into materials
    transform: Transform,
}

impl LoadedScene {
    fn spawn(&self, world: &mut World);
}
```

Compound assets (glTF, custom scene format) are loaded through the `AssetServer` and produce `LoadedScene` values that bulk-insert entities. `LoadedScene` is an _import-time_ product (DCC interchange); authored cells and saves use the OQ 20 document format — the two meet at spawn time, not on disk.

#### Worlds & Game Flow

_(OQ 33 resolution, 2026-08-12.)_ The flow state machine — MainMenu → Loading → Playing → Paused — is **game code** (an enum in the App, or a behavior); the engine never knows what a "main menu" is. It provides the primitives the flow composes:

- **Background world construction.** `begin_world_load(descriptor)` builds a successor World while the current one keeps running — a loading screen is just the current World: spawns from documents, streams cells, warms assets through the normal async paths. `world_load_progress()` exposes asset/cell readiness for progress bars.
- **Frame-boundary swap.** `swap_world(handle)` applies at end of frame: the old World tears down properly (behavior `shutdown`s, deferred despawns honored, emitters stop; the physics world rebuilds from the new World's descriptions per OQ 20), and the extract emits a **scene-reset delta** — the retained `RenderScene` clears wholesale and repopulates from the new World's first extract. GPU-resident assets are engine-level, not world-level: shared assets survive the swap via refcounts; only entities go.
- **Cross-swap audio falls out of the existing split.** `AudioCommands` voices live in the engine-side mixer and survive swaps — menu music persists into the loading screen; `AudioEmitter`s die with their entities. The imperative/declarative pairing encodes _lifetime_.
- **Pause & time scale.** `Time` carries `scale`, `paused`, and `real_dt`. Paused ⇒ the fixed accumulator freezes (zero ticks: physics and fixed gameplay halt) and `dt` reads 0, while `update` continues with `real_dt` so menus animate over a frozen world. `scale < 1` slows the accumulator: slow-motion with per-tick determinism intact. The engine does **not** auto-pause audio — ducking the gameplay bus is a game decision through the mixer.
- **Pause is the _in-game_ tool — a main menu is a live World.** An animated menu backdrop (birds, clouds, wind) is just a small World running normally with UI entities in front; nothing is paused because there is no gameplay to protect. `paused`/`scale` are **global engine state, not world state**: they persist across swaps, and the engine never implicitly pauses or unpauses anything — flow transitions that want a different time state set it explicitly.
- **Two clocks reach shaders too.** Frame uniforms carry both scaled `elapsed` and `real_elapsed`. Environmental shader effects (wind, water, cloud drift) default to **scaled** time — slow-mo bends the world, in-game pause freezes it — while real time is available per material for effects that must keep moving behind a pause menu.
- **Loading screens & transitions.** A loading screen is pure composition, nothing new: a live World (never paused) with an image widget, music via `AudioCommands` (seamless across swaps), and a progress bar driven by `world_load_progress()` each `update`. **Fade-to-black** needs zero machinery — a full-screen UI quad animates alpha on `real_dt`, the swap hides behind full black, fade back in. **Crossfade** uses the shipped freeze-frame pattern: `swap_world(handle, SwapTransition::CaptureLastFrame)` captures the old world's final presented frame into a texture handle, and the game's UI fades that static image out over the new world's live render — visually indistinguishable from a live crossfade for short transitions. True dual-world live crossfade (two full scene renders composited) is explicitly out of scope.
- **Runtime settings.** `EngineConfig` holds _initial_ values; `set_pacing()` and `set_window_mode()` mutate at frame boundaries through the existing control plumbing. A settings screen is UI + these setters + rebinding via the input action layer (see Input — Action Mapping).

#### Entity Hierarchy

Entities can form parent-child trees. A character entity might parent its weapon, particle emitters, and audio sources. When the parent moves, children inherit the transform. When the parent is despawned, children are despawned recursively.

This is equivalent to UE5's `SetupAttachment` component tree — but flattened into a simple parent ID on the entity rather than a component hierarchy.

#### Serialization & Save Games

_(OQ 20 resolution, 2026-08-11 — shape committed now, implemented when save games are first needed.)_ Persistence follows the opt-in registry model — the same shape as Godot 4's exported properties + `PackedScene` and Bevy's `DynamicScene`:

- **Opt-in component registry.** Components register for persistence under a **stable name + version**, with serde-derived (de)serialization and per-version migration hooks. The registry is shared infrastructure, not save-specific: replication (OQ 19) consumes the same component identity + codecs, and a future reflection layer (editor inspectors) backs the same registry without changing the save format. One registry, three consumers.
- **Save documents.** Header (engine + save versions) + entity section (registered components over a chosen entity set) + game-defined sections. Format-agnostic via serde — RON in dev (diffable), binary + compression shipping.
- **Loading spawns fresh entities.** `EntityId`s are never stable across sessions. Component fields that reference entities use the **`EntityRef` wrapper type**, so the loader knows every reference site and remaps automatically — the dangling-reference bug class is eliminated structurally, not documented around. Asset handles serialize as paths/UUIDs and re-resolve through the `AssetServer`.
- **Transient state is rebuilt, never saved.** Transforms re-propagate, GPU resources re-upload, behaviors re-`init`. **Discipline rule (load-bearing):** persistent state lives in components; behaviors and VMs hold only transient state — which is why Lua state never needs serializing. A dev-mode audit warns when a save touches unregistered component types, so opt-in silence can't bite silently.
- **Bulk world data is out of scope.** Voxel regions live in streaming-owned region files (OQ 17); a save _references_ region state, never inlines it. Saves stay small; worlds stay on disk.

### 4. Assets & Resources

#### AssetServer

Assets are loaded asynchronously and accessed via generation-counted handles.

```rust
struct AssetServer {
    registry: HashMap<AssetId, AssetEntry>,
    loaders:  Vec<Box<dyn AssetLoader>>,
    io_pool:  ThreadPool,
    watcher:  Option<FileWatcher>,  // hot-reload in dev builds
}

impl AssetServer {
    fn load<T: Asset>(&mut self, path: &str) -> AssetHandle<T>;
    fn state<T: Asset>(&self, handle: AssetHandle<T>) -> AssetState;
    fn get<T: Asset>(&self, handle: AssetHandle<T>) -> Option<&T>;
    fn unload<T: Asset>(&mut self, handle: AssetHandle<T>);
}

trait AssetLoader: Send + Sync {
    fn extensions(&self) -> &[&str];
    fn load(&self, bytes: &[u8], path: &Path) -> Result<Box<dyn Asset>>;
}

enum AssetState { Unloaded, Loading, Loaded, Failed(String) }
```

Games register custom asset _importers_ for game-specific source formats — the `AssetLoader` trait runs at **cook time** (dev cook-on-demand or the shipping cook; see Resource Pipeline & Filesystem), never at shipping load time. The engine provides built-in importers for meshes (glTF/GLB), textures (PNG, KTX2), audio (WAV, OGG), and scenes. Assets resolve through the VFS mounts and are identified by GUID (paths are human-facing aliases).

GPU-destined bulk data is decoded directly into staging-pool regions (see Staging Pool & Upload Path); the `AssetServer` retains CPU-side copies only for assets that need CPU access, so GPU-only assets cost no long-lived CPU memory.

#### Handles

```rust
struct AssetHandle<T> {
    id:         AssetId,
    generation: u32,
    _marker:    PhantomData<T>,
}

struct ResourceHandle<T> {
    id:         ResourceId,
    generation: u32,
    _marker:    PhantomData<T>,
}
```

- `AssetHandle<T>` — references an immutable, shared asset (mesh geometry, texture pixels, audio clip). Many entities can hold the same handle.
- `ResourceHandle<T>` — references a mutable resource in the World (materials). The game can modify these at runtime (e.g., animate a material's emissive intensity).

Both use generational indices for use-after-free detection.

#### Asset Types

```rust
struct MeshAsset {
    vertices: Vec<Vertex>,
    indices:  Vec<u32>,
    bounds:   AABB,
    lods:     Vec<MeshLod>,
}

struct TextureAsset {
    pixels: Vec<u8>,
    width:  u32,
    height: u32,
    format: TextureFormat,
    mips:   bool,
}

struct Vertex {
    position: [f32; 3],
    normal:   [f32; 3],
    uv:       [f32; 2],
    tangent:  [f32; 4],  // xyz + bitangent sign
}
```

### 5. Input

Accumulated per-frame on the main thread. Provides held/pressed/released semantics for digital inputs and continuous values for analog inputs.

```rust
struct Input {
    keyboard:    KeyboardState,
    mouse:       MouseState,
    controllers: [Option<ControllerState>; 4],
}

impl Input {
    fn key_held(&self, key: KeyCode) -> bool;
    fn key_pressed(&self, key: KeyCode) -> bool;      // true on the frame the key goes down
    fn key_released(&self, key: KeyCode) -> bool;     // true on the frame the key goes up
    fn mouse_position(&self) -> Vec2;
    fn mouse_delta(&self) -> Vec2;
    fn mouse_button_held(&self, button: MouseButton) -> bool;
    fn scroll_delta(&self) -> f32;
    fn controller(&self, index: usize) -> Option<&ControllerState>;
}

struct ControllerState {
    left_stick:  Vec2,
    right_stick: Vec2,
    left_trigger:  f32,
    right_trigger: f32,
    buttons: ControllerButtons,
}
```

#### Action Mapping

_(OQ 34 resolution, 2026-08-12.)_ The raw polling API above remains (tools, debug), but the game-facing standard is **actions**: named, rebindable, device-agnostic. Engine/game split: the engine owns the mapping machinery, the context stack, and binding persistence; games declare action maps as data.

```rust
struct ActionMap {                   // asset: "gameplay", "ui", "vehicle", …
    actions: Vec<ActionDef>,
    passthrough: bool,               // false = blocks maps below it on the stack
}

struct ActionDef {
    name: InternedString,            // "jump", "move", "fire"
    kind: ActionKind,                // Button | Axis1 | Axis2
    bindings: Vec<Binding>,          // rebindable; user edits serialize to user://
}

enum Binding {
    Key(KeyCode),
    MouseButton(MouseButton),
    MouseMotion,                                       // Axis2
    ControllerButton(u8),
    ControllerAxis { axis: u8, dead_zone: f32 },
    Composite2D { up: KeyCode, down: KeyCode, left: KeyCode, right: KeyCode },  // WASD
}

impl Input {
    fn action_held(&self, name: &str) -> bool;
    fn action_pressed(&self, name: &str) -> bool;      // frame edge — see fixed-tick rule
    fn action_released(&self, name: &str) -> bool;
    fn axis1(&self, name: &str) -> f32;
    fn axis2(&self, name: &str) -> Vec2;
    fn push_map(&mut self, map: AssetHandle<ActionMap>);
    fn pop_map(&mut self);
}
```

- **Context stack.** Active maps stack: opening the pause menu pushes the `ui` map, which (by default, `passthrough: false`) blocks gameplay actions beneath it — this is the input half of the UI focus/capture rule (OQ 18). Popping restores gameplay.
- **Rebinding & persistence.** `ActionMap`s are assets; user rebinds serialize to `user://` (the settings screen edits bindings; the VFS persists them).
- **Fixed-tick edge rule.** Action edges are per-_frame_, but a frame may run zero or two fixed ticks — so edges can be missed or double-seen by fixed logic. The documented pattern: read edges in `update`, convert to intent state ("jump requested"), consume the intent in `fixed_update`. The engine documents the pattern rather than hiding it.

### 6. Audio

Game code issues audio commands; the audio system runs on a dedicated thread. No direct API access from game code — same server pattern as the rendering thread. (The audio engine itself — mixer graph, voices, DSP, streaming — is specified in the top-level Audio section.)

```rust
struct AudioCommands {
    commands: Vec<AudioCommand>,
}

impl AudioCommands {
    fn play(&mut self, clip: AssetHandle<AudioClip>, params: PlayParams) -> SoundHandle;
    fn stop(&mut self, handle: SoundHandle);
    fn set_listener(&mut self, position: Vec3, forward: Vec3, up: Vec3);
    fn set_volume(&mut self, handle: SoundHandle, volume: f32);
    fn set_position(&mut self, handle: SoundHandle, position: Vec3);
}

struct PlayParams {
    volume:   f32,
    pitch:    f32,
    spatial:  bool,
    position: Vec3,
    looping:  bool,
}
```

### 7. Events

A typed event bus for decoupled communication between game systems. The bus is **double-buffered** _(OQ 6, 2026-08-11)_: events sent during frame N become readable during frame N+1 and are dropped at its end. `read<E>()` always returns the previous frame's events, so results are deterministic regardless of system ordering — no send-before-read hazards within a frame.

```rust
struct EventBus {
    channels: HashMap<TypeId, Box<dyn Any>>,
}

impl EventBus {
    fn send<E: Event>(&mut self, event: E);
    fn read<E: Event>(&self) -> &[E];
}
```

### 8. Change Tracking

The engine tracks which entities and components have been modified each frame. This drives the extract step — only dirty data is re-extracted for the Render Thread.

```rust
struct ChangeTracker {
    spawned:         HashSet<EntityId>,
    despawned:       HashSet<EntityId>,
    dirty:           HashMap<TypeId, HashSet<EntityId>>,  // per-component-type dirty sets
    dirty_resources: HashSet<ResourceId>,  // mutable resources (materials) — drives UpdateMaterial ops
}

impl ChangeTracker {
    fn is_dirty<C: Component>(&self, entity: EntityId) -> bool;
    fn dirty_set<C: Component>(&self) -> &HashSet<EntityId>;
    fn spawned(&self) -> &HashSet<EntityId>;
    fn despawned(&self) -> &HashSet<EntityId>;
}
```

Mutations through `World::get_mut<C>()` automatically mark the component dirty; mutations through a `ResourceHandle` (e.g., animating a material) mark the resource dirty. The change tracker is cleared after the extract step completes.
