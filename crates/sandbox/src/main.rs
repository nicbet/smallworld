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


fn make_box(size: Vec3) -> Mesh {
    let (hx, hy, hz) = (size.x * 0.5, size.y * 0.5, size.z * 0.5);
    let faces: &[([f32; 3], [[f32; 3]; 4])] = &[
        ([0.0, 1.0, 0.0],  [[-hx, hy, -hz], [hx, hy, -hz], [hx, hy, hz], [-hx, hy, hz]]),       // +Y
        ([0.0, -1.0, 0.0], [[-hx, -hy, hz], [hx, -hy, hz], [hx, -hy, -hz], [-hx, -hy, -hz]]),   // -Y
        ([0.0, 0.0, 1.0],  [[-hx, -hy, hz], [-hx, hy, hz], [hx, hy, hz], [hx, -hy, hz]]),        // +Z
        ([0.0, 0.0, -1.0], [[hx, -hy, -hz], [hx, hy, -hz], [-hx, hy, -hz], [-hx, -hy, -hz]]),    // -Z
        ([1.0, 0.0, 0.0],  [[hx, -hy, hz], [hx, hy, hz], [hx, hy, -hz], [hx, -hy, -hz]]),        // +X
        ([-1.0, 0.0, 0.0], [[-hx, -hy, -hz], [-hx, hy, -hz], [-hx, hy, hz], [-hx, -hy, hz]]),    // -X
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, corners) in faces {
        let base = vertices.len() as u32;
        for &pos in corners {
            vertices.push(Vertex {
                position: pos,
                normal: *normal,
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(vertices, indices)
}

fn populate_test_scene(world: &mut World) {
    // Sun — angled to cast visible shadows
    world.add_light(Light::directional(
        Vec3::new(0.3, -1.0, 0.2),
        Vec3::new(1.0, 0.98, 0.92),
        3.0,
    ));

    // Warm fill point light (visible attenuation)
    world.add_light(Light::point(
        Vec3::new(2.0, 3.0, -1.0),
        15.0,
        Vec3::new(1.0, 0.7, 0.4),
        8.0,
    ));

    // Cool rim spot light
    world.add_light(Light::spot(
        Vec3::new(-3.0, 4.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        12.0,
        0.3,
        0.5,
        Vec3::new(0.5, 0.7, 1.0),
        10.0,
    ));

    // -- Materials --

    let stone = world.add_material(Material {
        base_color: Vec4::new(0.45, 0.42, 0.38, 1.0),
        roughness: 0.85,
        metallic: 0.0,
        emissive: Vec3::ZERO,
    });

    let metal = world.add_material(Material {
        base_color: Vec4::new(0.8, 0.8, 0.85, 1.0),
        roughness: 0.15,
        metallic: 1.0,
        emissive: Vec3::ZERO,
    });

    let red_plastic = world.add_material(Material {
        base_color: Vec4::new(0.8, 0.15, 0.1, 1.0),
        roughness: 0.4,
        metallic: 0.0,
        emissive: Vec3::ZERO,
    });

    let wood = world.add_material(Material {
        base_color: Vec4::new(0.55, 0.35, 0.18, 1.0),
        roughness: 0.7,
        metallic: 0.0,
        emissive: Vec3::ZERO,
    });

    // -- Floor (20×20 metres) --

    let floor = world.add_mesh(Mesh::new(
        vec![
            Vertex { position: [-10.0, 0.0, -10.0], normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0], tangent: [1.0, 0.0, 0.0, 1.0] },
            Vertex { position: [10.0, 0.0, -10.0], normal: [0.0, 1.0, 0.0], uv: [1.0, 0.0], tangent: [1.0, 0.0, 0.0, 1.0] },
            Vertex { position: [10.0, 0.0, 10.0], normal: [0.0, 1.0, 0.0], uv: [1.0, 1.0], tangent: [1.0, 0.0, 0.0, 1.0] },
            Vertex { position: [-10.0, 0.0, 10.0], normal: [0.0, 1.0, 0.0], uv: [0.0, 1.0], tangent: [1.0, 0.0, 0.0, 1.0] },
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

    // -- Boxes (different sizes, materials, positions) --

    let unit_box = world.add_mesh(make_box(Vec3::ONE));

    // Tall stone pillar (casts shadow on floor)
    world.add_mesh_instance(MeshInstance {
        mesh: unit_box,
        material: stone,
        position: Vec3::new(0.0, 1.5, -2.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::new(0.6, 3.0, 0.6),
        casts_shadows: true,
    });

    // Metal cube (shiny, catches specular highlights)
    world.add_mesh_instance(MeshInstance {
        mesh: unit_box,
        material: metal,
        position: Vec3::new(2.0, 0.5, 0.0),
        rotation: Quat::from_rotation_y(0.4),
        scale: Vec3::splat(1.0),
        casts_shadows: true,
    });

    // Red plastic cube (saturated diffuse color)
    world.add_mesh_instance(MeshInstance {
        mesh: unit_box,
        material: red_plastic,
        position: Vec3::new(-1.5, 0.4, 1.0),
        rotation: Quat::from_rotation_y(-0.3),
        scale: Vec3::splat(0.8),
        casts_shadows: true,
    });

    // Wooden crate (medium roughness)
    world.add_mesh_instance(MeshInstance {
        mesh: unit_box,
        material: wood,
        position: Vec3::new(-3.0, 0.6, -1.5),
        rotation: Quat::from_rotation_y(0.7),
        scale: Vec3::new(1.2, 1.2, 1.2),
        casts_shadows: true,
    });

    // Small metal cube near the point light
    world.add_mesh_instance(MeshInstance {
        mesh: unit_box,
        material: metal,
        position: Vec3::new(2.5, 0.25, -1.0),
        rotation: Quat::from_rotation_y(1.0),
        scale: Vec3::splat(0.5),
        casts_shadows: true,
    });
}

