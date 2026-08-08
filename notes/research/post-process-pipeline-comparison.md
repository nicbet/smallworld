# Post-Process Pipeline Comparison: UE5 vs Godot 4 vs Unity

Date: 2026-08-08

## Pipeline Order

### UE5

Runs as a chain of separate RDG (Render Dependency Graph) passes:

1. Scene color compositing (translucency merged onto opaques)
2. **Depth of Field** (separate translucency merged afterward)
3. **Motion Blur**
4. **TAA / TSR** resolve
5. **Auto Exposure / Eye Adaptation** (64-bin luminance histogram)
6. **Bloom** + **Lens Flares** (from AA-resolved image, merged into tonemapper input)
7. **Tone Mapping** (HDR linear → LDR sRGB; bloom buffer added here)
8. **Color Grading** (applied inside the tonemapper pass via combined LUT)
9. Custom Post Process Materials (after tonemapping, receiving final LDR color)

### Godot 4

Fixed-order passes configured on the `Environment` resource:

1. **SSAO** (applied during/after lighting, before transparents — not truly post-process)
2. **SSR** (before transparents, Forward+ only — not truly post-process)
3. **SDFGI** (GI, computed before final composite)
4. **DOF** (near and far blur)
5. **Glow/Bloom**
6. **Tonemap** (including auto-exposure)
7. **Adjustments** (brightness, contrast, saturation, color correction)

SSAO/SSR/SDFGI run as part of the 3D rendering pass. The true post-process chain is: DOF → Glow → Tonemap → Adjustments.

### Unity URP

Two-stage structure (from `PostProcessPass.cs`):

1. Stop NaN propagation (if enabled)
2. SMAA (if selected, applied early)
3. **Depth of Field**
4. **Motion Blur**
5. Panini Projection
6. **Bloom**
7. **Uber pass** — lens distortion, chromatic aberration, vignette, color adjustments, tonemapping (combined via shader keywords in a single fullscreen blit)
8. **Final pass** — film grain, dithering, FXAA (if selected)

### Unity HDRP

1. Exposure
2. DLSS upsample (before post)
3. TAA or SMAA
4. **Depth of Field**
5. DLSS upsample (after DOF)
6. **Motion Blur**
7. Panini Projection
8. Lens Flare (SRP)
9. **Bloom**
10. **Uber pass** — color grading, lens distortion, chromatic aberration, vignette
11. DLSS upsample (after post)
12. **Final pass** — film grain, dithering, FXAA

Custom injection points: After Opaque And Sky, Before TAA, Before Post Process, After Post Process Blurs, After Post Process.

## Defaults

### UE5 — Most Opinionated

Even without a placed Post Process Volume, UE5 applies global defaults:

- **Auto Exposure**: ON (histogram-based, 64-bin)
- **Bloom**: ON
- **Tone Mapping**: ON (ACES Filmic)
- **Motion Blur**: ON (amount 0.5, max 5)
- **AA**: TSR (Temporal Super Resolution)
- **DOF**: Present but no visible effect until focal parameters configured
- Default lens attenuation for exposure: 0.78

### Godot 4 — Blank Canvas

All post-process effects disabled by default on a new Environment:

- `glow_enabled = false`
- `ssao_enabled = false`, `ssr_enabled = false`, `sdfgi_enabled = false`
- `dof_blur_far_enabled = false`, `dof_blur_near_enabled = false`
- `adjustment_enabled = false`
- `auto_exposure_enabled = false`
- `tonemap_mode = TONE_MAP_LINEAR` (no curve applied)
- No AA enabled

### Unity — Opt-In Everything

Camera post-processing toggle is ON by default (URP 14+), but no Volume Overrides are active:

- No tonemapping until override added
- No bloom, DOF, motion blur until override added
- No AA by default (URP); TAA in HDRP templates
- Template scenes may ship pre-configured Volumes, but blank scenes have nothing

## Tone Mapping

| Engine   | Default            | Options                              |
|----------|--------------------|--------------------------------------|
| UE5      | **ACES Filmic**    | ACES (custom via post-process mats)  |
| Godot 4  | **Linear** (none)  | Linear, Reinhard, Filmic, ACES       |
| Unity    | **None**           | Neutral, ACES                        |

### UE5 ACES Filmic

S-curve response modeled on the Academy Color Encoding System. Simulates film stock response. Applied in a single fused pass that also applies the color grading LUT. Default since UE 4.15.

### Godot 4 Linear

No tone curve. HDR values pass through without contrast adjustment. The ACES option exists but must be explicitly selected.

### Unity Neutral vs ACES

Neutral does minimal hue/saturation shift (range remapping only). ACES produces more cinematic, contrasty look with stronger hue shifts. Neither is applied by default.

## Auto Exposure

| Engine   | Default | Method                                                    |
|----------|---------|-----------------------------------------------------------|
| UE5      | **ON**  | 64-bin luminance histogram, hybrid linear/exponential adaptation |
| Godot 4  | **OFF** | Configured via CameraAttributes resource; scale=0.4, speed=0.5 when enabled |
| Unity    | **OFF** | HDRP: Fixed/Auto/Curve/Physical Camera modes; URP: indirect via Color Adjustments post-exposure |

### UE5 Histogram Details

- Metering mode: Auto Exposure Histogram (default) or Auto Exposure Basic (average luminance)
- Adaptation: hybrid linear/exponential curve, configurable rate (f-stops/second)
- Default exponential transition distance: 1.5 f-stops
- Simpler "Basic" mode available using average luminance

## Anti-Aliasing

| Engine      | Default   | Options                     |
|-------------|-----------|-----------------------------|
| UE5         | **TSR**   | TSR, TAA, FXAA              |
| Godot 4     | **None**  | MSAA, FXAA, TAA (Fwd+ only)|
| Unity URP   | **None**  | FXAA, SMAA, TAA, MSAA       |
| Unity HDRP  | **TAA**   | TAA, SMAA, FXAA              |

TSR (UE5) renders at lower internal resolution and reconstructs higher-resolution output using temporal history — simultaneously anti-aliasing and upscaling. Less ghosting than legacy TAA.

## Bloom / Glow

### UE5

Computed from the AA-resolved image. Merged into the tonemapper input buffer (applied before tonemapping in linear HDR space). On by default.

### Godot 4

Thresholded progressive downsampling. Pixels above `glow_hdr_threshold` (default 1.0) contribute. `glow_bloom` parameter (0.0 default) allows sub-threshold glow. Up to 7 mip levels, each toggleable. Blend modes: Additive, Screen, Softlight, Replace. Default intensity 0.8, strength 1.0. Disabled by default.

### Unity

Separate pass before the uber pass. Uses progressive downsampling + upsampling (dual Kawase or similar). Merged into the uber pass input. Disabled by default.

## Architecture

### UE5 — RDG Pass Chain + Post Process Volumes

Each effect is its own RDG pass with declared input/output textures. Post Process Volumes are axis-aligned bounding boxes (or unbounded with "Infinite Extent") that hold override settings. Multiple overlapping volumes blend by priority (higher wins) and blend weight (0–1). Custom multi-pass effects via Scene View Extensions (hook into renderer, allocate intermediate textures, chain RDG passes).

### Godot 4 — Environment Resource + Compositor

Single `Environment` resource configures all effects as toggles + parameters. Fixed execution order, not user-configurable. Compositor system (4.3+) adds custom `CompositorEffect` hooks at 4 injection points:

- `EFFECT_CALLBACK_TYPE_PRE_OPAQUE`
- `EFFECT_CALLBACK_TYPE_POST_OPAQUE`
- `EFFECT_CALLBACK_TYPE_PRE_TRANSPARENT`
- `EFFECT_CALLBACK_TYPE_POST_TRANSPARENT` (before built-in post-processing)

Each CompositorEffect implements `_render_callback()` with custom compute shaders via RenderingDevice.

### Unity — Volume Framework + Uber Pass

Heavy effects (DOF, bloom, motion blur) run as separate passes. Lightweight effects batched into a single uber pass (shader keywords, one fullscreen blit) to minimize render-target switches and bandwidth. A second final pass handles grain, dithering, FXAA.

Volume framework: Volume component → Volume Profile (ScriptableObject) → list of Volume Overrides. Global or Local volumes with blend distance and priority. Camera position determines interpolation. HDRP uses compute shaders; URP uses fragment shaders.

## Color Grading

| Engine  | Mode            | LUT Support                                  |
|---------|-----------------|----------------------------------------------|
| UE5     | HDR (fused with tonemap) | Combined LUT baked into tonemapper pass |
| Godot 4 | Post-tonemap adjustments | Color correction via adjustment textures |
| Unity URP | LDR or HDR (configurable) | LDR: 2D LUT post-tonemap; HDR: 3D log LUT pre-tonemap |
| Unity HDRP | Always HDR    | Procedural (lift/gamma/gain, curves, etc.) + external LUT import; default LUT size 32 |

## Key Observations for Engine Design

1. **Uber pass batching** (Unity) is the most GPU-efficient for lightweight effects — fusing tonemap + color grading + vignette + adjustments into one fullscreen blit saves RT switches.

2. **Separate passes** for expensive spatial effects (bloom, DOF, motion blur) is universal — all three engines do this.

3. **ACES tonemap on by default** (UE5) is the single biggest visual quality win out of the box.

4. **Auto-exposure off by default** is the consensus — even UE5's default can surprise users. Godot and Unity both default it off.

5. **Volume/Environment-based configuration** is the standard pattern — per-camera or spatial blending of post-process parameters.

6. **Bloom before tonemap** is standard — operating in linear HDR space produces more physically plausible results.

## Sources

- [Post Process Effects in UE 5.8 — Epic](https://dev.epicgames.com/documentation/en-us/unreal-engine/post-process-effects-in-unreal-engine)
- [Color Grading and the Filmic Tonemapper — Epic](https://dev.epicgames.com/documentation/en-us/unreal-engine/color-grading-and-the-filmic-tonemapper-in-unreal-engine)
- [Auto Exposure in UE — Epic](https://dev.epicgames.com/documentation/en-us/unreal-engine/auto-exposure-in-unreal-engine)
- [Anti-Aliasing and Upscaling in UE 5.8 — Epic](https://dev.epicgames.com/documentation/en-us/unreal-engine/anti-aliasing-and-upscaling-in-unreal-engine)
- [Environment and Post-Processing — Godot 4.4 docs](https://docs.godotengine.org/en/4.4/tutorials/3d/environment_and_post_processing.html)
- [The Compositor — Godot 4.4 docs](https://docs.godotengine.org/en/4.4/tutorials/rendering/compositor.html)
- [HDRP Post-Processing Execution Order (v17)](https://docs.unity3d.com/Packages/com.unity.render-pipelines.high-definition@17.0/manual/rendering-execution-order.html)
- [URP PostProcessPass.cs source — GitHub](https://github.com/Unity-Technologies/Graphics/blob/master/Packages/com.unity.render-pipelines.universal/Runtime/Passes/PostProcessPass.cs)
- [URP Introduction to Post-Processing — Unity 6](https://docs.unity3d.com/6000.5/Documentation/Manual/urp/integration-with-post-processing.html)
