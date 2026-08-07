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

use crate::gpu::GpuContext;
use crate::input::Input;
use crate::world::{World, WorldGpuData};

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

/// Configuration for engine initialization.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Window title.
    pub title: String,
    /// Window mode and dimensions.
    pub window_mode: WindowMode,
    /// Enable vsync (maps to `PresentMode::AutoVsync` vs `AutoNoVsync`).
    pub vsync: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            title: "smallworld".to_string(),
            window_mode: WindowMode::default(),
            vsync: true,
        }
    }
}

/// Surface + its configuration, bundled as one concept.
struct DisplaySurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// Thin wrapper around a surface texture for the current frame.
///
/// Keeps `wgpu::SurfaceTexture` out of game code's vocabulary.
pub struct FrameContext {
    surface_texture: wgpu::SurfaceTexture,
    view: wgpu::TextureView,
}

impl FrameContext {
    /// The texture view for render pass color attachment.
    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

/// Top-level engine struct. Owns GPU context, display surface, and the
/// cached World→GPU extraction result.
pub struct Engine {
    window: Option<Arc<Window>>,
    gpu: GpuContext,
    display: Option<DisplaySurface>,
    gpu_data: WorldGpuData,
    input: Input,
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

        Self {
            window: Some(window),
            gpu,
            display: Some(DisplaySurface {
                surface,
                config: surface_config,
            }),
            gpu_data: WorldGpuData::empty(),
            input: Input::default(),
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
            gpu_data: WorldGpuData::empty(),
            input: Input::default(),
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

    /// Extracts world data if dirty and acquires the next surface frame.
    ///
    /// Returns `None` if the frame should be skipped (surface timeout,
    /// occluded, or headless). The game renders between `begin_frame` and
    /// [`present`](Self::present).
    pub fn begin_frame(&mut self, world: &mut World) -> Option<FrameContext> {
        if world.is_dirty() {
            self.gpu_data = WorldGpuData::extract(&self.gpu.device, world);
            world.clear_dirty();
        }

        let display = self.display.as_mut()?;

        match display.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => {
                let view = tex
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                Some(FrameContext {
                    surface_texture: tex,
                    view,
                })
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                display.surface.configure(&self.gpu.device, &display.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => None,
            other => {
                log::error!("surface error: {other:?}");
                None
            }
        }
    }

    /// Presents a frame acquired from [`begin_frame`](Self::begin_frame).
    pub fn present(&self, frame: FrameContext) {
        self.gpu.queue.present(frame.surface_texture);
    }

    /// The cached World→GPU extraction result. Always valid — empty until
    /// the first `begin_frame` with a non-empty world.
    #[must_use]
    pub fn gpu_data(&self) -> &WorldGpuData {
        &self.gpu_data
    }

    /// The current frame's input snapshot. Stable for the entire update call.
    #[must_use]
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The GPU context. Transitional — subsystems that take `&GpuContext`
    /// use this until they move behind Engine.
    #[must_use]
    pub fn gpu(&self) -> &GpuContext {
        &self.gpu
    }

    /// The logical device.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.gpu.device
    }

    /// The command submission queue.
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.gpu.queue
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
/// The engine calls [`update`](App::update) then [`render`](App::render)
/// once per frame. Other lifecycle methods have default no-op implementations.
pub trait App {
    /// Called once per frame. Mutate the world, read input, advance game state.
    fn update(&mut self, engine: &mut Engine, world: &mut World, dt: f32);

    /// Called once per frame with the surface texture view. Submit GPU work here.
    /// Default: no-op (clears to black).
    #[allow(unused_variables)]
    fn render(&mut self, engine: &mut Engine, frame: &FrameContext) {}
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
        log::info!(
            "engine: {} ({:?})",
            engine.adapter_info().name,
            engine.adapter_info().backend,
        );
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

                if let Some(frame) = state.engine.begin_frame(&mut state.world) {
                    self.app.render(&mut state.engine, &frame);
                    state.engine.present(frame);
                }
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
