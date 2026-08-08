//! Engine: top-level struct owning GPU plumbing and the World→GPU bridge.
//!
//! Games create an [`Engine`] with an [`EngineConfig`], mutate the [`World`],
//! and call [`begin_frame`](Engine::begin_frame) /
//! [`present`](Engine::present). The engine handles surface management,
//! extraction, and all wgpu internals.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::cull::CullStage;
use crate::gbuffer::GBufferPass;
use crate::gpu::GpuContext;
use crate::input::Input;
use crate::jobs::RayonScheduler;
use crate::lighting::LightingPass;
use crate::stream::{PlaceholderExtractor, StreamStage};
use crate::world::World;

/// Window mode for engine initialization.
#[derive(Clone, Debug)]
pub enum WindowMode {
    /// Windowed with logical size.
    Windowed {
        /// Window width in logical pixels.
        width: u32,
        /// Window height in logical pixels.
        height: u32,
    },
    /// Placeholder — not yet implemented.
    Fullscreen,
}

impl Default for WindowMode {
    fn default() -> Self {
        Self::Windowed {
            width: 1280,
            height: 720,
        }
    }
}

/// Log level for engine output.
#[derive(Clone, Copy, Debug, Default)]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Errors and warnings.
    Warn,
    /// Errors, warnings, and informational messages (default).
    #[default]
    Info,
    /// Verbose debug output.
    Debug,
    /// Maximum verbosity including per-frame traces.
    Trace,
}

impl LogLevel {
    fn to_filter(self) -> log::LevelFilter {
        match self {
            Self::Error => log::LevelFilter::Error,
            Self::Warn => log::LevelFilter::Warn,
            Self::Info => log::LevelFilter::Info,
            Self::Debug => log::LevelFilter::Debug,
            Self::Trace => log::LevelFilter::Trace,
        }
    }
}

/// Configuration for engine initialization.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Window title.
    pub title: String,
    /// Window mode and dimensions.
    pub window_mode: WindowMode,
    /// Enable vsync (maps to `PresentMode::AutoVsync` vs `AutoNoVsync`).
    pub vsync: bool,
    /// Log level. Overridden by `SMALLWORLD_LOG` env var if set.
    pub log_level: LogLevel,
    /// Worker thread count for the engine's internal job pool.
    /// `0` = auto-detect from hardware (default).
    pub worker_threads: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            title: "smallworld".to_string(),
            window_mode: WindowMode::default(),
            vsync: true,
            log_level: LogLevel::default(),
            worker_threads: 0,
        }
    }
}

fn init_logger(level: LogLevel) {
    use std::io::Write;

    let filter = if let Ok(env_level) = std::env::var("SMALLWORLD_LOG") {
        match env_level.to_lowercase().as_str() {
            "error" => log::LevelFilter::Error,
            "warn" => log::LevelFilter::Warn,
            "info" => log::LevelFilter::Info,
            "debug" => log::LevelFilter::Debug,
            "trace" => log::LevelFilter::Trace,
            _ => level.to_filter(),
        }
    } else {
        level.to_filter()
    };
    env_logger::Builder::new()
        .filter_level(filter)
        .format(|buf, record| {
            let ts = buf.timestamp_millis();
            let level = record.level();
            let module = record.module_path().unwrap_or("unknown");
            writeln!(
                buf,
                "[{ts}] [smallworld] [{module}] [{level}] {}",
                record.args()
            )
        })
        .init();
}

/// Surface + its configuration, bundled as one concept.
struct DisplaySurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// View parameters set by the game each frame.
#[derive(Clone, Copy, Debug)]
pub struct ViewState {
    /// Camera world-space position.
    pub position: glam::Vec3,
    /// Horizontal rotation in radians.
    pub yaw: f32,
    /// Vertical rotation in radians.
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            position: glam::Vec3::new(0.0, 2.0, 5.0),
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60.0_f32.to_radians(),
        }
    }
}

/// Top-level engine struct. Owns GPU context, display surface, renderer,
/// and the cached World→GPU extraction result.
pub struct Engine {
    window: Option<Arc<Window>>,
    gpu: GpuContext,
    display: Option<DisplaySurface>,
    input: Input,
    view: ViewState,
    cull_stage: CullStage,
    stream_stage: StreamStage,
    gbuffer_pass: Option<GBufferPass>,
    lighting_pass: Option<LightingPass>,
    jobs: RayonScheduler,
}

impl Engine {
    /// Creates an engine with a window. Call from winit's `resumed` callback.
    pub fn new(config: EngineConfig, event_loop: &ActiveEventLoop) -> Self {
        let (width, height) = match config.window_mode {
            WindowMode::Windowed { width, height } => (width, height),
            WindowMode::Fullscreen => (1920, 1080),
        };

        let attrs = WindowAttributes::default()
            .with_title(&config.title)
            .with_inner_size(winit::dpi::LogicalSize::new(width, height));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        log::info!("boot: creating GPU context");
        let instance = GpuContext::create_instance();
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");
        let gpu = pollster::block_on(GpuContext::new(instance, &surface));

        let size = window.inner_size();
        let surface_config = gpu.configure_surface(&surface, size.width.max(1), size.height.max(1));

        let present_mode = if config.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let mut surface_config = surface_config;
        if surface_config.present_mode != present_mode {
            surface_config.present_mode = present_mode;
            surface.configure(&gpu.device, &surface_config);
        }

        let vsync_label = if config.vsync { "on" } else { "off" };
        log::info!(
            "boot: surface {}x{} {:?} vsync={}",
            surface_config.width,
            surface_config.height,
            surface_config.format,
            vsync_label,
        );

        let inner = window.inner_size();
        let w = inner.width.max(1);
        let h = inner.height.max(1);
        let gbuffer_pass = GBufferPass::new(&gpu.device, &gpu.queue, surface_config.format, w, h);
        log::info!("boot: gbuffer pass ready");

        let lighting_pass = LightingPass::new(&gpu.device, surface_config.format, w, h);
        log::info!("boot: lighting pass ready");

        let jobs = if config.worker_threads > 0 {
            RayonScheduler::new(config.worker_threads)
        } else {
            RayonScheduler::auto()
        };

        Self {
            window: Some(window),
            gpu,
            display: Some(DisplaySurface {
                surface,
                config: surface_config,
            }),
            input: Input::default(),
            view: ViewState::default(),
            cull_stage: CullStage::new(),
            stream_stage: StreamStage::new(Arc::new(PlaceholderExtractor)),
            gbuffer_pass: Some(gbuffer_pass),
            lighting_pass: Some(lighting_pass),
            jobs,
        }
    }

    /// Creates a headless engine for CI and testing — no window, no surface.
    pub fn headless() -> Self {
        let instance = GpuContext::create_instance();
        let gpu = pollster::block_on(GpuContext::headless(instance));
        Self {
            window: None,
            gpu,
            display: None,
            input: Input::default(),
            jobs: RayonScheduler::auto(),
            view: ViewState::default(),
            cull_stage: CullStage::new(),
            stream_stage: StreamStage::new(Arc::new(PlaceholderExtractor)),
            gbuffer_pass: None,
            lighting_pass: None,
        }
    }

    /// Reconfigures the display surface at a new size.
    pub fn resize(&mut self, width: u32, height: u32) {
        let Some(display) = &mut self.display else {
            return;
        };
        let w = width.max(1);
        let h = height.max(1);
        display.config.width = w;
        display.config.height = h;
        display.surface.configure(&self.gpu.device, &display.config);
        if let Some(g) = &mut self.gbuffer_pass {
            g.resize(&self.gpu.device, w, h);
        }
        if let Some(l) = &mut self.lighting_pass {
            l.resize(&self.gpu.device, w, h);
        }
    }

    /// Changes the vsync mode and reconfigures the surface.
    pub fn set_vsync(&mut self, enabled: bool) {
        let Some(display) = &mut self.display else {
            return;
        };
        let mode = if enabled {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if display.config.present_mode != mode {
            display.config.present_mode = mode;
            display.surface.configure(&self.gpu.device, &display.config);
        }
    }

    /// Sets the viewpoint the engine renders from. Call each frame from
    /// your update function after computing camera position.
    pub fn set_camera(&mut self, position: glam::Vec3, yaw: f32, pitch: f32) {
        self.view.position = position;
        self.view.yaw = yaw;
        self.view.pitch = pitch;
    }

    /// Sets the vertical field of view in radians.
    pub fn set_fov(&mut self, fov_y: f32) {
        self.view.fov_y = fov_y;
    }

    /// Extracts world data if dirty, renders, and presents. Called by the
    /// engine loop — games never call this directly.
    fn render_frame(&mut self, world: &mut World) {
        let _changes = world.drain_changes();
        let visibility = self.cull_stage.cull(world, &self.view, None);
        let stream_output = self.stream_stage.stream(
            world,
            &visibility,
            &self.jobs,
            &self.gpu.device,
            self.view.position,
        );

        let Some(display) = self.display.as_mut() else {
            return;
        };

        let frame = match display.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                display.surface.configure(&self.gpu.device, &display.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            other => {
                log::error!("surface error: {other:?}");
                return;
            }
        };

        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (w, h) = (display.config.width, display.config.height);
        let aspect = w as f32 / h.max(1) as f32;
        let camera = crate::camera::FreeCamera {
            position: self.view.position,
            yaw: self.view.yaw,
            pitch: self.view.pitch,
            fov_y: self.view.fov_y,
            aspect,
            near: 0.1,
            far: 1000.0,
        };

        let mut encoder =
            self.gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame"),
                });

        if let Some(gbuffer_pass) = &mut self.gbuffer_pass {
            gbuffer_pass.render(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &camera,
                world,
                &stream_output,
            );

            if let Some(lighting_pass) = &mut self.lighting_pass {
                lighting_pass.render(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &mut encoder,
                    &surface_view,
                    gbuffer_pass.gbuffer(),
                    &camera,
                    world,
                    &visibility,
                    &stream_output,
                );
            }
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        self.gpu.queue.present(frame);
    }

    /// The current frame's input snapshot. Stable for the entire update call.
    #[must_use]
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The surface texture format, or `Rgba8Unorm` for headless.
    #[must_use]
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.display
            .as_ref()
            .map(|d| d.config.format)
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm)
    }

    /// The current surface dimensions, or `(0, 0)` for headless.
    #[must_use]
    pub fn surface_size(&self) -> (u32, u32) {
        self.display
            .as_ref()
            .map(|d| (d.config.width, d.config.height))
            .unwrap_or((0, 0))
    }

    /// The window, if windowed.
    #[must_use]
    pub fn window(&self) -> Option<&Window> {
        self.window.as_deref()
    }

    /// Adapter metadata for debug display.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.gpu.adapter_info()
    }

    /// Whether the device supports GPU timestamp queries.
    #[must_use]
    pub fn supports_timestamps(&self) -> bool {
        self.gpu.supports_timestamps()
    }

    /// Runs the engine loop. The game implements [`App`] to provide its
    /// state and update logic. The engine handles window creation, event
    /// dispatch, resize, close, frame acquire/present.
    ///
    /// ```ignore
    /// struct Game { camera: CameraRig }
    ///
    /// impl App for Game {
    ///     fn update(&mut self, engine: &mut Engine, world: &mut World, dt: f32) {
    ///         self.camera.update(engine.input(), dt);
    ///     }
    /// }
    ///
    /// Engine::run(EngineConfig::default(), World::new(), Game { .. });
    /// ```
    pub fn run(config: EngineConfig, world: World, app: impl App + 'static) {
        init_logger(config.log_level);
        log::info!("smallworld engine {}", crate::VERSION);

        let event_loop = EventLoop::new().expect("failed to create event loop");
        let mut runner = AppRunner {
            config: Some(config),
            world: Some(world),
            state: None,
            app: Box::new(app),
        };
        event_loop.run_app(&mut runner).expect("event loop error");
    }
}

/// Trait for game logic. Implement this on your game state struct.
///
/// The engine calls [`update`](App::update) once per frame, then renders
/// automatically from the view set via [`Engine::set_camera`].
pub trait App {
    /// Called once per frame. Read input, advance game state, call
    /// [`Engine::set_camera`] to position the viewpoint.
    fn update(&mut self, engine: &mut Engine, world: &mut World, dt: f32);
}

struct AppRunnerState {
    engine: Engine,
    world: World,
    last_frame: Instant,
}

struct AppRunner {
    config: Option<EngineConfig>,
    world: Option<World>,
    state: Option<AppRunnerState>,
    app: Box<dyn App>,
}

impl ApplicationHandler for AppRunner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let config = self.config.take().unwrap_or_default();
        let engine = Engine::new(config, event_loop);
        log::info!("game loop started");
        self.state = Some(AppRunnerState {
            engine,
            world: self.world.take().unwrap_or_default(),
            last_frame: Instant::now(),
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                state
                    .engine
                    .resize(new_size.width.max(1), new_size.height.max(1));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if event.state == ElementState::Pressed && key == KeyCode::Escape {
                        event_loop.exit();
                        return;
                    }
                    state
                        .engine
                        .input
                        .on_keyboard(key, event.state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                state
                    .engine
                    .input
                    .on_mouse_button(button, btn_state == ElementState::Pressed);
            }
            WindowEvent::CursorMoved { position, .. } => {
                state
                    .engine
                    .input
                    .on_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;

                self.app.update(&mut state.engine, &mut state.world, dt);

                state.engine.input.begin_frame();
                state.engine.render_frame(&mut state.world);
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let Some(state) = self.state.as_mut()
            && let winit::event::DeviceEvent::MouseMotion { delta: (dx, dy) } = event
        {
            state.engine.input.on_mouse_motion(dx as f32, dy as f32);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_ref()
            && let Some(w) = state.engine.window()
        {
            w.request_redraw();
        }
    }
}
