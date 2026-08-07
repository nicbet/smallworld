//! Sandbox: dev/test harness for the smallworld engine.

mod camera_rig;

use camera_rig::CameraRig;
use smallworld_engine::engine::{App, Engine, EngineConfig, FrameContext};
use smallworld_engine::placeholder::PlaceholderRenderer;
use smallworld_engine::world::World;

struct Game {
    camera: CameraRig,
    renderer: Option<PlaceholderRenderer>,
}

impl App for Game {
    fn update(&mut self, engine: &mut Engine, _world: &mut World, dt: f32) {
        self.camera.update(engine.input(), dt);

        let (w, h) = engine.surface_size();
        match &mut self.renderer {
            None => {
                self.renderer = Some(PlaceholderRenderer::new(
                    engine.device(),
                    engine.surface_format(),
                    w.max(1),
                    h.max(1),
                ));
            }
            Some(r) => r.resize(engine.device(), w.max(1), h.max(1)),
        }
    }

    fn render(&mut self, engine: &mut Engine, frame: &FrameContext) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let mut encoder = engine.device().create_command_encoder(
            &smallworld_engine::wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            },
        );
        renderer.render(
            engine.queue(),
            &mut encoder,
            frame.view(),
            &self.camera.camera,
        );
        engine.queue().submit(std::iter::once(encoder.finish()));
    }
}

fn main() {
    env_logger::init();

    Engine::run(
        EngineConfig::default(),
        World::new(),
        Game {
            camera: CameraRig::new(),
            renderer: None,
        },
    );
}
