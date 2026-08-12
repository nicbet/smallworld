## Resource Pipeline & Filesystem

_(OQ 27 resolution, 2026-08-11.)_ The offline half of assets; the runtime half (AssetServer, staging pool, streaming) is specified elsewhere. Engine/game split: the pipeline, database, VFS, and caches are pure engine; games contribute custom importers and cooked formats.

### Asset Identity: GUIDs, Paths as Aliases

Every asset receives a **stable GUID at import**, stored in a sidecar `.meta` file that travels with the source in version control — the Unity-proven shape. Every serialized reference (saves, cells, descriptors, inter-asset dependencies) is a GUID; every human interaction is a path; the asset database maps between them. Renames and moves never break references. (This resolves OQ 20's "paths/UUIDs" ambiguity: **GUID on disk, path in hands.**)

### Cooking: Asset Database + Derived-Data Cache

Source assets (glTF, PNG, WAV, …) are **imported into engine-native cooked artifacts** — meshes in final vertex layout, textures BC/ASTC-compressed (KTX2), clips ACL-compressed — keyed by `(source content hash, importer version)` in a derived-data cache. The same cache-key discipline as `GenerationPolicy::CacheToDisk`, applied to imports.

- **Dev builds cook on demand.** First use imports transparently and caches; edits re-cook via the file watcher (hot reload rides this). Team-shared caches are a later drop-in.
- **Shipping builds run a full cook** via the `smallworld-cook` CLI, which shares importer code with the engine. The shipping runtime contains **no importers** — it reads cooked formats only, which are memcpy-shaped and decode straight into staging regions.
- **The `AssetLoader` trait is relocated.** It is the _importer_ trait, running at cook time (dev-transparent or CLI), never at shipping load time. Games register importers for custom source formats and define custom cooked formats.

### Filesystem: a Thin VFS with Mounts

- **`content://`** — cooked assets. Loose files in dev; **pak archives** in shipping (zstd-compressed, index + blobs).
- **`user://`** — saves, settings, region overlays: the canonical home for the OQ 17 / OQ 20 write targets.
- **`temp://`** — scratch.

Engine and game code address assets only through mounts; platform paths resolve once at mount time. Mod support is structurally reserved — a mod is a higher-priority mount — noted, not designed.

### Dependencies & Unload Policy

Import records each asset's dependency edges (material → textures, anim graph → clips) in the asset database; a load request pulls its dependency closure through the normal async path. Handles refcount, and **zero references makes an asset _evictable_, not evicted** — actual eviction is the budget arbiter's call under memory pressure: the GPU-is-a-cache invariant extended to CPU-side assets. `AssetServer::unload()` is an eviction _hint_, not choreography.
