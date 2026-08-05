//! Debug viewer and application shell for the smallworld engine.

use std::sync::Arc;
use std::time::Instant;

use smallworld_engine::camera::FreeCamera;
use smallworld_engine::gpu::GpuContext;
use smallworld_engine::wgpu;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

fn main() {
    env_logger::init();

    if std::env::args().any(|a| a == "--info") {
        print_adapter_info();
        return;
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop error");
}

/// Print adapter information and exit (for CI smoke tests).
fn print_adapter_info() {
    let instance = GpuContext::create_instance();
    let ctx = pollster::block_on(GpuContext::headless(instance));
    let info = ctx.adapter_info();
    println!("smallworld-viewer {}", env!("CARGO_PKG_VERSION"));
    println!("  engine      {}", smallworld_engine::VERSION);
    println!("  adapter     {}", info.name);
    println!("  backend     {:?}", info.backend);
    println!("  driver      {}", info.driver);
}

// ---------------------------------------------------------------------------
// Input state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InputState {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    sprint: bool,
    right_mouse: bool,
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct App {
    state: Option<RunState>,
}

struct RunState {
    window: Arc<Window>,
    gpu: GpuContext,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    camera: FreeCamera,
    input: InputState,
    last_frame: Instant,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("smallworld")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));
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

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            surface_config.format,
            egui_wgpu::RendererOptions::default(),
        );

        let aspect = size.width as f32 / size.height.max(1) as f32;
        let camera = FreeCamera::new(aspect);

        self.state = Some(RunState {
            window,
            gpu,
            surface,
            surface_config,
            egui_ctx,
            egui_state,
            egui_renderer,
            camera,
            input: InputState::default(),
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

        let egui_response = state.egui_state.on_window_event(&state.window, &event);
        if egui_response.consumed {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(new_size) => {
                let w = new_size.width.max(1);
                let h = new_size.height.max(1);
                state.surface_config = state.gpu.configure_surface(&state.surface, w, h);
                state.camera.aspect = w as f32 / h as f32;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyW) => state.input.forward = pressed,
                    PhysicalKey::Code(KeyCode::KeyS) => state.input.backward = pressed,
                    PhysicalKey::Code(KeyCode::KeyA) => state.input.left = pressed,
                    PhysicalKey::Code(KeyCode::KeyD) => state.input.right = pressed,
                    PhysicalKey::Code(KeyCode::KeyQ | KeyCode::Space) => state.input.up = pressed,
                    PhysicalKey::Code(KeyCode::KeyE | KeyCode::ControlLeft) => {
                        state.input.down = pressed;
                    }
                    PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight) => {
                        state.input.sprint = pressed;
                    }
                    PhysicalKey::Code(KeyCode::Escape) if pressed => event_loop.exit(),
                    _ => {}
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                if button == MouseButton::Right {
                    state.input.right_mouse = btn_state == ElementState::Pressed;
                }
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;

                // Camera movement
                let speed = FreeCamera::BASE_SPEED
                    * if state.input.sprint {
                        FreeCamera::SPRINT_MULTIPLIER
                    } else {
                        1.0
                    }
                    * dt;
                let mut delta = glam::Vec3::ZERO;
                if state.input.forward {
                    delta.z += speed;
                }
                if state.input.backward {
                    delta.z -= speed;
                }
                if state.input.right {
                    delta.x += speed;
                }
                if state.input.left {
                    delta.x -= speed;
                }
                if state.input.up {
                    delta.y += speed;
                }
                if state.input.down {
                    delta.y -= speed;
                }
                state.camera.translate(delta);

                // Run egui every frame so texture deltas are always consumed.
                let raw_input = state.egui_state.take_egui_input(&state.window);
                let mut full_output = state.egui_ctx.run_ui(raw_input, |ctx| {
                    draw_debug_panel(ctx, state);
                });
                state
                    .egui_state
                    .handle_platform_output(&state.window, full_output.platform_output);
                let paint_jobs = state
                    .egui_ctx
                    .tessellate(full_output.shapes, full_output.pixels_per_point);

                let screen_desc = egui_wgpu::ScreenDescriptor {
                    size_in_pixels: [state.surface_config.width, state.surface_config.height],
                    pixels_per_point: full_output.pixels_per_point,
                };

                // Acquire surface
                let frame = match state.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
                    wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                        let size = state.window.inner_size();
                        state.surface_config = state.gpu.configure_surface(
                            &state.surface,
                            size.width.max(1),
                            size.height.max(1),
                        );
                        None
                    }
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => None,
                    other => {
                        log::error!("surface error: {other:?}");
                        None
                    }
                };

                let mut encoder =
                    state
                        .gpu
                        .device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("frame"),
                        });

                for (id, deltas) in full_output.textures_delta.set.drain() {
                    for delta in &deltas {
                        state.egui_renderer.update_texture(
                            &state.gpu.device,
                            &state.gpu.queue,
                            id,
                            delta,
                        );
                    }
                }
                let cmd_buffers = state.egui_renderer.update_buffers(
                    &state.gpu.device,
                    &state.gpu.queue,
                    &mut encoder,
                    &paint_jobs,
                    &screen_desc,
                );

                if let Some(ref frame) = frame {
                    let view = frame
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());

                    let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("main"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.1,
                                    g: 0.1,
                                    b: 0.12,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    state.egui_renderer.render(
                        &mut rpass.forget_lifetime(),
                        &paint_jobs,
                        &screen_desc,
                    );
                }

                for id in full_output.textures_delta.free.drain() {
                    state.egui_renderer.free_texture(&id);
                }

                state.gpu.queue.submit(
                    cmd_buffers
                        .into_iter()
                        .chain(std::iter::once(encoder.finish())),
                );
                if let Some(frame) = frame {
                    state.gpu.queue.present(frame);
                }
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && state.input.right_mouse
        {
            state.camera.rotate(
                dx as f32 * FreeCamera::SENSITIVITY,
                -dy as f32 * FreeCamera::SENSITIVITY,
            );
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_ref() {
            state.window.request_redraw();
        }
    }
}

// ---------------------------------------------------------------------------
// Debug overlay
// ---------------------------------------------------------------------------

fn draw_debug_panel(ctx: &egui::Context, state: &RunState) {
    let info = state.gpu.adapter_info();
    let dt = state.last_frame.elapsed().as_secs_f32();

    egui::Window::new("Debug")
        .default_pos([8.0, 8.0])
        .default_width(240.0)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.label(format!("Adapter: {} — {:?}", info.name, info.backend));
            ui.label(format!("Driver: {}", info.driver));
            ui.separator();
            ui.label(format!(
                "Window: {}×{}",
                state.surface_config.width, state.surface_config.height
            ));
            ui.separator();
            ui.label(format!(
                "Pos: ({:.1}, {:.1}, {:.1})",
                state.camera.position.x, state.camera.position.y, state.camera.position.z,
            ));
            ui.label(format!(
                "Yaw: {:.1}°  Pitch: {:.1}°",
                state.camera.yaw.to_degrees(),
                state.camera.pitch.to_degrees(),
            ));
            ui.separator();
            if dt > 0.0 {
                ui.label(format!(
                    "Frame: {:.2} ms ({:.0} FPS)",
                    dt * 1000.0,
                    1.0 / dt
                ));
            }
        });
}
