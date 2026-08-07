//! Sandbox: dev/test harness for the smallworld engine.

mod camera_rig;

use camera_rig::CameraRig;
use smallworld_engine::engine::{App, Engine, EngineConfig};
use smallworld_engine::world::World;

struct Game {
    camera: CameraRig,
}

impl App for Game {
    fn update(&mut self, engine: &mut Engine, _world: &mut World, dt: f32) {
        self.camera.update(engine, dt);
    }
}

fn main() {
    env_logger::init();

    Engine::run(
        EngineConfig::default(),
        World::new(),
        Game {
            camera: CameraRig::new(),
        },
    );
}
