# Shadow Atlas Sizing Across Engines

Research for sw-dcc28a (shadow atlas + light grid + deferred shade).

## Unity

### HDRP
- **Default:** 4096x4096, three separate atlases (punctual, area, directional)
- **Range:** Powers of two, configurable in HDRP Asset
- **Subdivision:** Dynamic packing. A 4096 atlas fits 16x 1024 maps, or 2x 2048 + 4x 1024 + 8x 512 + 32x 256. Supports Dynamic Rescale for non-cached maps
- **Memory:** ~64 MB per atlas at D32F (4096x4096x4 bytes); ~192 MB total for three atlases at default

### URP
- **Default:** 2048x2048 (main light), 2048x2048 (additional lights atlas)
- **Range:** 256-4096
- **Memory:** ~16 MB per atlas at D16, ~32 MB at D32

## Unreal Engine 5

- **Default:** Virtual Shadow Maps (VSM), 16k x 16k virtual resolution per light
- **Page size:** 128x128 pixels, allocated on demand from depth buffer analysis
- **Subdivision:** No fixed atlas grid; pages are cached and invalidated per-frame
- **Memory:** Dynamic; only rendered pages consume memory
- **Legacy:** Traditional shadow maps (512-4096 atlas) still available as fallback

## Godot 4

- **Default:** 4096x4096
- **Range:** Power of two, configurable in Project Settings
- **Subdivision:** Atlas split into 4 quadrants. Default subdivisions: Q0=4 maps, Q1=4, Q2=16, Q3=64 (total 88 shadow slots)
- **Memory:** ~64 MB at D32F, ~32 MB at D16
- **Tradeoff:** Fixed quadrant system is simple but inflexible

## Decision

4096x4096 single atlas (64 MB at D32F). Matches Unity HDRP and Godot defaults. Atlas subdivision with simple grid packing (not quadrants) to support multiple shadow-casting lights from the start.

## Sources

- [HDRP Shadows docs](https://docs.unity3d.com/Packages/com.unity.render-pipelines.high-definition@17.1/manual/Shadows-in-HDRP.html)
- [URP Shadows docs](https://docs.unity3d.com/6000.1/Documentation/Manual/urp/shadow-resolution-urp.html)
- [VSM docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/virtual-shadow-maps-in-unreal-engine)
- [Godot lights and shadows](https://docs.godotengine.org/en/stable/tutorials/3d/lights_and_shadows.html)
