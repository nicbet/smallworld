//! Sandbox: dev/test harness for the smallworld engine.

mod camera_rig;

use camera_rig::CameraRig;
use glam::{Quat, Vec3, Vec4};
use smallworld_engine::engine::{App, Engine, EngineConfig};
use smallworld_engine::light::Light;
use smallworld_engine::material::Material;
use smallworld_engine::mesh::{Mesh, MeshInstance, Vertex};
use smallworld_engine::world::World;

struct Game {
    camera: CameraRig,
}

impl App for Game {
    fn update(&mut self, engine: &mut Engine, _world: &mut World, dt: f32) {
        self.camera.update(engine, dt);
    }
}

fn populate_test_scene(world: &mut World) {
    // Sun
    world.add_light(Light::directional(
        Vec3::new(0.3, -1.0, 0.2),
        Vec3::new(1.0, 0.98, 0.92),
        3.0,
    ));

    // Warm fill light
    world.add_light(Light::point(
        Vec3::new(2.0, 3.0, -1.0),
        15.0,
        Vec3::new(1.0, 0.7, 0.4),
        8.0,
    ));

    // Stone material
    let stone = world.add_material(Material {
        base_color: Vec4::new(0.45, 0.42, 0.38, 1.0),
        roughness: 0.85,
        metallic: 0.0,
        emissive: Vec3::ZERO,
    });

    // Floor quad (two triangles, 10×10 metres)
    let floor = world.add_mesh(Mesh::new(
        vec![
            Vertex {
                position: [-5.0, 0.0, -5.0],
                normal: [0.0, 1.0, 0.0],
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            },
            Vertex {
                position: [5.0, 0.0, -5.0],
                normal: [0.0, 1.0, 0.0],
                uv: [1.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            },
            Vertex {
                position: [5.0, 0.0, 5.0],
                normal: [0.0, 1.0, 0.0],
                uv: [1.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            },
            Vertex {
                position: [-5.0, 0.0, 5.0],
                normal: [0.0, 1.0, 0.0],
                uv: [0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            },
        ],
        vec![0, 1, 2, 0, 2, 3],
    ));

    world.add_mesh_instance(MeshInstance {
        mesh: floor,
        material: stone,
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
        casts_shadows: false,
    });
}

fn main() {
    let mut world = World::new();
    populate_test_scene(&mut world);

    Engine::run(
        EngineConfig::default(),
        world,
        Game {
            camera: CameraRig::new(),
        },
    );
}
