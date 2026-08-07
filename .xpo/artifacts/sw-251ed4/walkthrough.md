## What was built

Structured engine logging with configurable log levels. The engine initializes the logger during `Engine::run` — games never call `env_logger::init()`. Boot phases are logged automatically.

## How it works

**Log format:** `[timestamp] [smallworld] [module_path] [LEVEL] message`

The module path comes automatically from Rust's `log` macros — `log::info!` in `engine.rs` produces `smallworld_engine::engine`, in `gpu.rs` produces `smallworld_engine::gpu`. No manual tagging needed.

**Config:** `EngineConfig::log_level` (default: `Info`). `SMALLWORLD_LOG` env var overrides without recompile.

**Boot output:**
```
[..] [smallworld] [smallworld_engine::engine] [INFO] smallworld engine 0.1.0
[..] [smallworld] [smallworld_engine::engine] [INFO] boot: creating GPU context
[..] [smallworld] [smallworld_engine::gpu]    [INFO] adapter: Apple M1 Max (Metal)
[..] [smallworld] [smallworld_engine::engine] [INFO] boot: surface 2560x1440 Bgra8UnormSrgb vsync=on
[..] [smallworld] [smallworld_engine::engine] [INFO] boot: placeholder renderer ready
[..] [smallworld] [smallworld_engine::engine] [INFO] game loop started
```

## Key decisions

- `env_logger` moved from sandbox to engine — logging is engine infrastructure, not game plumbing.
- `LogLevel` is our own enum so games don't need a `log` dependency for configuration.
- Duplicate adapter logging removed — `GpuContext::new` already logs adapter info with the `gpu` module tag, so the boot sequence doesn't repeat it.
