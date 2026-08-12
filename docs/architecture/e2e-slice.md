### M4 — The Sandbox Slice (E2E Acceptance)

The vertical slice that proves the engine end to end, at each subsystem's _minimum viable
tier_: boot → live main menu → settings → loading screen → playtest level → pause → save →
back to menu → quit. Every beat traces to specs; the slice is the acceptance test for the
branch tracks' first rungs.

| Beat                         | Exercises                                                                   | Sufficient tier                                                                                                                                                              |
| ---------------------------- | --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Boot to main menu            | M0–M3 core; World from documents                                            | —                                                                                                                                                                            |
| 3D backdrop: mountain        | Mesh path (M1), retained scene                                              | Static meshes (voxel terrain = stretch goal via the plugin)                                                                                                                  |
| Clouds wafting               | Sky                                                                         | **Skybox interim tier** (panning textures) — the clouds module is _not_ required                                                                                             |
| Birds flying                 | `BehaviorHost`                                                              | One Rust bird + one **Lua bird** — proves both tiers behave identically                                                                                                      |
| Sun rays through clouds      | Froxel volumetrics + shadow atlas + directional light                       | Crepuscular rays fall out of froxels — no extra feature                                                                                                                      |
| Music playing                | Audio: commands, mixer buses                                                | In-memory clip acceptable; streaming music = bonus                                                                                                                           |
| Menus, dialogs, progress bar | UI                                                                          | **Interim decision: egui for the slice's UI** (buttons/sliders/dialogs today); widgets-as-entities replaces it in its own round — the slice must not pull that round forward |
| Settings: key bindings       | Action mapping (OQ 34) + rebind persistence to `user://`                    | Full — this _is_ the OQ 34 acceptance test                                                                                                                                   |
| Settings: audio volumes      | Per-bus volume (MixerLayout)                                                | Full                                                                                                                                                                         |
| Settings: graphics           | `set_pacing` (vsync, DRS toggle), `set_window_mode`                         | Full — runtime settings (OQ 33) acceptance                                                                                                                                   |
| New game / load save         | OQ 20 registry + save documents                                             | Minimal registered-component set                                                                                                                                             |
| Loading screen + progress    | `begin_world_load` / `world_load_progress` / `swap_world` (OQ 33)           | Full, including fade or freeze-frame transition                                                                                                                              |
| Run around                   | KCC (OQ 28) + camera rig + action axes                                      | Full move-and-slide, slopes, grounding                                                                                                                                       |
| Jump                         | KCC vertical + grounding + **the frame-edge → intent → fixed-tick pattern** | The documented input gotcha, exercised deliberately                                                                                                                          |
| Push a box                   | KCC impulses → dynamic bodies (rapier)                                      | Full                                                                                                                                                                         |
| Something falls              | Gravity, dynamic bodies, collision vs. level                                | Full                                                                                                                                                                         |
| Pause menu                   | `set_paused`, `real_dt` UI, bus ducking                                     | Full — frozen world behind animated UI                                                                                                                                       |
| Save / back to menu / quit   | OQ 20 save; reverse `swap_world` (music surviving); teardown protocol       | Full                                                                                                                                                                         |

**Deliberately not exercised** (each is a later slice, not scope creep here): the Voxel Plugin
(stretch goal only), both streaming layers (the playtest level is a single unstreamed World),
GI clipmap, RT, TAAU/DRS under load, skeletal animation (the character may be a capsule),
decals, probes, vegetation/PCG, networking. A second slice ("the planet walk") exercises
streaming + voxels + LOD transitions when those tracks land.
