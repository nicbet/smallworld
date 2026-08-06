//! Sandbox: dev/test harness for the smallworld engine.

mod bench;
mod model_gen;
mod scenes;
mod worldgen;

use std::sync::Arc;
use std::time::Instant;

use smallworld_engine::brick_index::BrickIndex;
use smallworld_engine::brick_pool::BrickPool;
use smallworld_engine::camera::FreeCamera;
use smallworld_engine::gpu::GpuContext;
use smallworld_engine::gpu_timing::GpuTimestamps;
use smallworld_engine::raymarcher::Raymarcher;
use smallworld_engine::scene::Scene;
use smallworld_engine::wgpu;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use scenes::Preset;

fn main() {
    env_logger::init();

    if std::env::args().any(|a| a == "--info") {
        print_adapter_info();
        return;
    }

    let bench_config = bench::parse_args();

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new(bench_config);
    event_loop.run_app(&mut app).expect("event loop error");
}

/// Print adapter information and exit (for CI smoke tests).
fn print_adapter_info() {
    let instance = GpuContext::create_instance();
    let ctx = pollster::block_on(GpuContext::headless(instance));
    let info = ctx.adapter_info();
    println!("smallworld-sandbox {}", env!("CARGO_PKG_VERSION"));
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
// Frame time history
// ---------------------------------------------------------------------------

const FRAME_HISTORY_LEN: usize = 300;

#[derive(Clone, Copy, Default)]
struct FrameSample {
    dt_ms: f32,
    cpu_ms: f32,
    gpu_ms: f32,
}

struct FrameHistory {
    samples: [FrameSample; FRAME_HISTORY_LEN],
    write: usize,
    count: usize,
}

impl FrameHistory {
    fn new() -> Self {
        Self {
            samples: [FrameSample::default(); FRAME_HISTORY_LEN],
            write: 0,
            count: 0,
        }
    }

    fn push(&mut self, sample: FrameSample) {
        self.samples[self.write] = sample;
        self.write = (self.write + 1) % FRAME_HISTORY_LEN;
        if self.count < FRAME_HISTORY_LEN {
            self.count += 1;
        }
    }

    fn iter_newest_first(&self) -> impl Iterator<Item = &FrameSample> {
        let start = (self.write + FRAME_HISTORY_LEN - 1) % FRAME_HISTORY_LEN;
        (0..self.count).map(move |i| {
            let idx = (start + FRAME_HISTORY_LEN - i) % FRAME_HISTORY_LEN;
            &self.samples[idx]
        })
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct App {
    state: Option<RunState>,
    bench_config: Option<bench::BenchConfig>,
}

struct RunState {
    window: Arc<Window>,
    gpu: GpuContext,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    brick_pool: BrickPool,
    brick_index: BrickIndex,
    scene: Scene,
    raymarcher: Raymarcher,
    timestamps: Option<GpuTimestamps>,
    camera: FreeCamera,
    current_preset: Preset,
    input: InputState,
    render_scale: f32,
    shadows: bool,
    smooth_normals: bool,
    last_frame: Instant,
    frame_history: FrameHistory,
    bench: Option<bench::BenchState>,
}

impl App {
    fn new(bench_config: Option<bench::BenchConfig>) -> Self {
        Self {
            state: None,
            bench_config,
        }
    }
}

impl RunState {
    fn load_preset(&mut self, preset: Preset) {
        self.brick_pool = BrickPool::new(&self.gpu.device, preset.pool_capacity());
        self.brick_index =
            BrickIndex::new(&self.gpu.device, preset.grid_dims(), preset.world_min());
        self.scene = Scene::new();
        preset.setup(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.brick_pool,
            &mut self.brick_index,
            &mut self.scene,
        );
        self.scene.upload(&self.gpu.device);

        let rw = ((self.surface_config.width as f32) * self.render_scale) as u32;
        let rh = ((self.surface_config.height as f32) * self.render_scale) as u32;
        self.raymarcher = Raymarcher::new(
            &self.gpu,
            rw.max(1),
            rh.max(1),
            self.surface_config.format,
            &self.brick_pool,
            &self.brick_index,
            &self.scene,
        );

        let (pos, yaw, pitch) = preset.camera_start();
        self.camera.position = pos;
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
        self.current_preset = preset;

        log::info!("loaded preset: {}", preset.label());
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

        let timestamps = if gpu.supports_timestamps() {
            Some(GpuTimestamps::new(&gpu.device, &gpu.queue, 3))
        } else {
            None
        };

        let preset = self
            .bench_config
            .as_ref()
            .map(|b| b.preset)
            .unwrap_or(Preset::Default);
        let mut brick_pool = BrickPool::new(&gpu.device, preset.pool_capacity());
        let mut brick_index = BrickIndex::new(&gpu.device, preset.grid_dims(), preset.world_min());
        let mut scene = Scene::new();
        preset.setup(
            &gpu.device,
            &gpu.queue,
            &mut brick_pool,
            &mut brick_index,
            &mut scene,
        );
        scene.upload(&gpu.device);

        let render_scale = if window.scale_factor() > 1.0 {
            0.5
        } else {
            1.0
        };
        let render_w = ((size.width.max(1) as f32) * render_scale) as u32;
        let render_h = ((size.height.max(1) as f32) * render_scale) as u32;

        let raymarcher = Raymarcher::new(
            &gpu,
            render_w.max(1),
            render_h.max(1),
            surface_config.format,
            &brick_pool,
            &brick_index,
            &scene,
        );

        let aspect = size.width as f32 / size.height.max(1) as f32;
        let (cam_pos, cam_yaw, cam_pitch) = preset.camera_start();
        let mut camera = FreeCamera::new(aspect);
        camera.position = cam_pos;
        camera.yaw = cam_yaw;
        camera.pitch = cam_pitch;

        let bench_state = self.bench_config.take().map(|config| {
            let mut bs = bench::BenchState::new(config, preset);
            bs.advance_orbit(0.0, &mut camera);
            bs
        });

        self.state = Some(RunState {
            window,
            gpu,
            surface,
            surface_config,
            egui_ctx,
            egui_state,
            egui_renderer,
            brick_pool,
            brick_index,
            scene,
            raymarcher,
            timestamps,
            camera,
            current_preset: preset,
            input: InputState::default(),
            render_scale,
            shadows: true,
            smooth_normals: false,
            last_frame: Instant::now(),
            frame_history: FrameHistory::new(),
            bench: bench_state,
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
                let rw = ((w as f32) * state.render_scale) as u32;
                let rh = ((h as f32) * state.render_scale) as u32;
                state.raymarcher.resize(
                    &state.gpu,
                    rw.max(1),
                    rh.max(1),
                    &state.brick_pool,
                    &state.brick_index,
                    &state.scene,
                );
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
                let frame_start = Instant::now();

                // GPU timestamp readback (previous frame)
                if let Some(ts) = &mut state.timestamps {
                    ts.read_results(&state.gpu.device);
                }

                // Delta time
                let now = Instant::now();
                let dt = (now - state.last_frame).as_secs_f32();
                state.last_frame = now;

                // Camera movement
                if let Some(bs) = &mut state.bench {
                    bs.advance_orbit(dt, &mut state.camera);
                } else {
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
                }

                // Run egui every frame so texture deltas are always consumed.
                let raw_input = state.egui_state.take_egui_input(&state.window);
                let mut render_scale = state.render_scale;
                let mut shadows = state.shadows;
                let mut smooth_normals = state.smooth_normals;
                let mut selected_preset = state.current_preset;
                let mut full_output = state.egui_ctx.run_ui(raw_input, |ctx| {
                    draw_debug_panel(
                        ctx,
                        state,
                        &mut render_scale,
                        &mut shadows,
                        &mut smooth_normals,
                        &mut selected_preset,
                    );
                    draw_frame_graph(ctx, &state.frame_history);
                });
                state.shadows = shadows;
                state.smooth_normals = smooth_normals;
                if selected_preset != state.current_preset {
                    state.load_preset(selected_preset);
                }
                if (render_scale - state.render_scale).abs() > 0.001 {
                    state.render_scale = render_scale;
                    let rw = ((state.surface_config.width as f32) * render_scale) as u32;
                    let rh = ((state.surface_config.height as f32) * render_scale) as u32;
                    state.raymarcher.resize(
                        &state.gpu,
                        rw.max(1),
                        rh.max(1),
                        &state.brick_pool,
                        &state.brick_index,
                        &state.scene,
                    );
                }
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

                    let compute_ts = state
                        .timestamps
                        .as_ref()
                        .map(|ts| ts.compute_pass_writes(0));
                    let blit_ts = state.timestamps.as_ref().map(|ts| ts.render_pass_writes(1));
                    let egui_ts = state.timestamps.as_ref().map(|ts| ts.render_pass_writes(2));

                    let mut flags = 0u32;
                    if state.shadows {
                        flags |= Raymarcher::FLAG_SHADOWS;
                    }
                    if state.smooth_normals {
                        flags |= Raymarcher::FLAG_SMOOTH_NORMALS;
                    }
                    state.raymarcher.render(
                        &state.gpu,
                        &mut encoder,
                        &view,
                        &state.camera,
                        &state.brick_index,
                        &state.scene,
                        flags,
                        compute_ts,
                        blit_ts,
                    );

                    let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                            depth_slice: None,
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: egui_ts,
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

                if let Some(ts) = &state.timestamps {
                    ts.resolve(&mut encoder);
                }

                state.gpu.queue.submit(
                    cmd_buffers
                        .into_iter()
                        .chain(std::iter::once(encoder.finish())),
                );
                if let Some(frame) = frame {
                    state.gpu.queue.present(frame);
                }

                // Record frame sample
                let cpu_ms = (Instant::now() - frame_start).as_secs_f32() * 1000.0;
                let gpu_averages = state
                    .timestamps
                    .as_ref()
                    .map(|ts| {
                        let a = ts.averages();
                        [a[0], a[1], a[2]]
                    })
                    .unwrap_or([0.0; 3]);
                let gpu_ms = gpu_averages.iter().sum::<f64>() as f32;
                state.frame_history.push(FrameSample {
                    dt_ms: dt * 1000.0,
                    cpu_ms,
                    gpu_ms,
                });

                if let Some(bs) = &mut state.bench {
                    bs.push_sample(bench::BenchSample {
                        dt_ms: dt * 1000.0,
                        cpu_ms,
                        gpu_compute_ms: gpu_averages[0],
                        gpu_blit_ms: gpu_averages[1],
                        gpu_egui_ms: gpu_averages[2],
                    });
                    if bs.is_done() {
                        bs.print_report(
                            state.brick_pool.live_count(),
                            state.brick_pool.capacity(),
                            state.scene.instance_count(),
                        );
                        event_loop.exit();
                    }
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
        if state.bench.is_some() {
            return;
        }
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

fn draw_debug_panel(
    ctx: &egui::Context,
    state: &RunState,
    render_scale: &mut f32,
    shadows: &mut bool,
    smooth_normals: &mut bool,
    selected_preset: &mut Preset,
) {
    let info = state.gpu.adapter_info();
    let dt = state.last_frame.elapsed().as_secs_f32();

    egui::Window::new("Debug")
        .default_pos([8.0, 8.0])
        .default_width(240.0)
        .collapsible(true)
        .show(ctx, |ui| {
            egui::ComboBox::from_label("Scene")
                .selected_text(selected_preset.label())
                .show_ui(ui, |ui| {
                    for &p in Preset::ALL {
                        ui.selectable_value(selected_preset, p, p.label());
                    }
                });
            ui.separator();
            ui.label(format!("Adapter: {} — {:?}", info.name, info.backend));
            ui.label(format!("Driver: {}", info.driver));
            ui.separator();
            let rw = ((state.surface_config.width as f32) * *render_scale) as u32;
            let rh = ((state.surface_config.height as f32) * *render_scale) as u32;
            ui.label(format!(
                "Window: {}×{}  Render: {}×{}",
                state.surface_config.width, state.surface_config.height, rw, rh
            ));
            ui.add(
                egui::Slider::new(render_scale, 0.25..=1.0)
                    .text("Scale")
                    .step_by(0.05),
            );
            ui.horizontal(|ui| {
                ui.checkbox(shadows, "Shadows");
                ui.checkbox(smooth_normals, "Smooth N");
            });
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
            ui.separator();
            ui.label(format!(
                "Bricks: {} / {}  Objects: {}",
                state.brick_pool.live_count(),
                state.brick_pool.capacity(),
                state.scene.instance_count(),
            ));
            ui.separator();
            if let Some(ts) = &state.timestamps {
                let avg = ts.averages();
                let names = ["Compute", "Blit", "egui"];
                let mut total = 0.0;
                for (name, &ms) in names.iter().zip(avg.iter()) {
                    ui.label(format!("GPU {name}: {ms:.2} ms"));
                    total += ms;
                }
                ui.label(format!("GPU Total: {total:.2} ms"));
            } else {
                ui.label("GPU: N/A");
            }
        });
}

// ---------------------------------------------------------------------------
// Frame time graph
// ---------------------------------------------------------------------------

const GRAPH_HEIGHT: f32 = 80.0;
const BAR_WIDTH: f32 = 2.0;
const TARGET_60: f32 = 16.67;
const TARGET_30: f32 = 33.33;
const COLOR_CPU: egui::Color32 = egui::Color32::from_rgb(100, 180, 255);
const COLOR_GPU: egui::Color32 = egui::Color32::from_rgb(80, 220, 160);
const COLOR_DT: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const COLOR_OVER_BUDGET: egui::Color32 = egui::Color32::from_rgb(240, 80, 80);

fn draw_frame_graph(ctx: &egui::Context, history: &FrameHistory) {
    egui::Window::new("Frame Time")
        .default_pos([8.0, 280.0])
        .default_width(FRAME_HISTORY_LEN as f32 * BAR_WIDTH + 16.0)
        .collapsible(true)
        .default_open(false)
        .show(ctx, |ui| {
            if history.count == 0 {
                ui.label("No data yet");
                return;
            }

            let latest =
                history.samples[(history.write + FRAME_HISTORY_LEN - 1) % FRAME_HISTORY_LEN];
            ui.label(format!(
                "dt {:.1} ms  cpu {:.1} ms  gpu {:.1} ms",
                latest.dt_ms, latest.cpu_ms, latest.gpu_ms
            ));

            let available_width = ui.available_width();
            let bar_count = ((available_width / BAR_WIDTH) as usize).min(history.count);

            let max_ms = history
                .iter_newest_first()
                .take(bar_count)
                .map(|s| s.dt_ms.max(s.gpu_ms))
                .fold(TARGET_60, f32::max)
                .max(1.0);

            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(bar_count as f32 * BAR_WIDTH, GRAPH_HEIGHT),
                egui::Sense::hover(),
            );
            let painter = ui.painter_at(rect);

            // Background
            painter.rect_filled(rect, 2.0, egui::Color32::from_gray(30));

            // Reference lines
            let y_for = |ms: f32| rect.bottom() - (ms / max_ms) * rect.height();

            if TARGET_60 < max_ms {
                let y = y_for(TARGET_60);
                painter.line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_premultiplied(100, 255, 100, 60),
                    ),
                );
                painter.text(
                    egui::pos2(rect.right() - 2.0, y - 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    "60",
                    egui::FontId::proportional(9.0),
                    egui::Color32::from_gray(120),
                );
            }
            if TARGET_30 < max_ms {
                let y = y_for(TARGET_30);
                painter.line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_premultiplied(255, 100, 100, 60),
                    ),
                );
                painter.text(
                    egui::pos2(rect.right() - 2.0, y - 2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    "30",
                    egui::FontId::proportional(9.0),
                    egui::Color32::from_gray(120),
                );
            }

            // Bars — newest on the right
            for (i, sample) in history.iter_newest_first().take(bar_count).enumerate() {
                let x = rect.right() - (i as f32 + 1.0) * BAR_WIDTH;

                // GPU bar (behind)
                let gpu_h = (sample.gpu_ms / max_ms) * rect.height();
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.bottom() - gpu_h),
                        egui::vec2(BAR_WIDTH - 0.5, gpu_h),
                    ),
                    0.0,
                    COLOR_GPU,
                );

                // CPU bar (in front, slightly narrower)
                let cpu_h = (sample.cpu_ms / max_ms) * rect.height();
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, rect.bottom() - cpu_h),
                        egui::vec2(BAR_WIDTH - 0.5, cpu_h),
                    ),
                    0.0,
                    COLOR_CPU.gamma_multiply(0.7),
                );

                // dt tick mark on top
                let dt_y = rect.bottom() - (sample.dt_ms / max_ms) * rect.height();
                let color = if sample.dt_ms > TARGET_30 {
                    COLOR_OVER_BUDGET
                } else {
                    COLOR_DT
                };
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, dt_y),
                        egui::vec2(BAR_WIDTH - 0.5, 1.5),
                    ),
                    0.0,
                    color,
                );
            }

            // Legend
            if response.hovered() {
                response.on_hover_ui(|ui| {
                    ui.horizontal(|ui| {
                        legend_dot(ui, COLOR_DT, "dt (wall)");
                        legend_dot(ui, COLOR_GPU, "GPU");
                        legend_dot(ui, COLOR_CPU.gamma_multiply(0.7), "CPU");
                    });
                });
            }
        });
}

fn legend_dot(ui: &mut egui::Ui, color: egui::Color32, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
    ui.label(egui::RichText::new(label).size(10.0));
}
