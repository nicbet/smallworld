## What

Engine initializes the logger and logs its boot phases. Games set the log level in `EngineConfig` instead of calling `env_logger::init()` manually.

## Why

Currently the sandbox calls `env_logger::init()` — that's engine plumbing leaked into game code. The engine should own logging just like it owns the GPU and window.

## Acceptance Criteria

- [ ] `EngineConfig` gains `log_level: LogLevel` (default: Info)
- [ ] `Engine::run` initializes the logger before anything else
- [ ] Engine logs boot phases: adapter name/backend, surface format/size, renderer init
- [ ] Engine logs game loop start
- [ ] `SMALLWORLD_LOG` env var overrides config level (for debugging without recompile)
- [ ] Sandbox removes `env_logger::init()` — engine handles it
- [ ] `make test` and `make lint` pass

## Design

### LogLevel

```rust
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

Maps to `log::LevelFilter`. Kept as our own enum so games don't need a `log` dependency.

### Boot sequence logging

```
[INFO  smallworld_engine::engine] boot: adapter Apple M3 Max (Metal)
[INFO  smallworld_engine::engine] boot: surface 1280×720 Bgra8UnormSrgb vsync=on
[INFO  smallworld_engine::engine] boot: placeholder renderer ready
[INFO  smallworld_engine::engine] game loop started
```

### Env override

`SMALLWORLD_LOG=debug` overrides `EngineConfig::log_level`. This lets developers crank up logging without changing code — just set the env var.

## Flow

1. Add `log_level` to `EngineConfig` with `LogLevel` enum
2. Add `env_logger` to engine deps (move from sandbox-only)
3. `Engine::run` calls logger init before creating the event loop
4. Add structured `log::info!` calls to `Engine::new` boot sequence
5. Remove `env_logger::init()` from sandbox
6. Remove `env_logger` dep from sandbox `Cargo.toml`
