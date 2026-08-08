//! Asset loading and path resolution.
//!
//! Runtime data — textures, material palettes, saved worlds — lives in the workspace's
//! `assets/` directory. Shaders do not: they are engine source and are baked into the
//! binary (see [`crate::shaders`]).
//!
//! Everything goes through [`root`] and [`path`] so the same tree is found whether the
//! binary was launched by `cargo run` from an arbitrary directory or shipped next to its
//! assets. Resolution order, first match wins:
//!
//! 1. `$SMALLWORLD_ASSETS` — explicit override, for packaging and tests.
//! 2. `<directory of the running executable>/assets` — the shipped layout.
//! 3. `<workspace root>/assets` — the development layout, derived from the compile-time
//!    `CARGO_MANIFEST_DIR`.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::material::Material;
use crate::mesh::{Mesh, MeshInstance, Vertex};
use crate::texture::TextureData;
use crate::world::{MeshInstanceKey, TextureKey, World};

/// Environment variable that overrides asset-root discovery.
pub const ASSET_ROOT_ENV: &str = "SMALLWORLD_ASSETS";

/// This crate's directory at compile time, i.e. `<workspace root>/crates/engine`.
const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The directory runtime assets are loaded from.
///
/// Resolved on first call and cached for the life of the process, so a mid-run change to
/// `$SMALLWORLD_ASSETS` has no effect.
#[must_use]
pub fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let exe_dir = env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        resolve(
            env::var_os(ASSET_ROOT_ENV).map(PathBuf::from),
            exe_dir.as_deref(),
        )
    })
}

/// Resolves `relative` against the [`root`].
///
/// `relative` is always written with `/` separators regardless of platform; it is split
/// into components before joining, so the result is correct on Windows too.
#[must_use]
pub fn path(relative: &str) -> PathBuf {
    let mut resolved = root().to_path_buf();
    resolved.extend(relative.split('/').filter(|part| !part.is_empty()));
    resolved
}

/// The resolution rule itself, kept free of environment access so it is testable.
///
/// Edition 2024 makes `env::set_var` unsafe (and it races across test threads), so the
/// inputs are passed in rather than read here.
fn resolve(override_dir: Option<PathBuf>, exe_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir;
    }
    if let Some(candidate) = exe_dir.map(|dir| dir.join("assets"))
        && candidate.is_dir()
    {
        return candidate;
    }
    dev_root()
}

/// `<workspace root>/assets`, derived from this crate's compile-time manifest directory.
fn dev_root() -> PathBuf {
    let manifest_dir = Path::new(MANIFEST_DIR);
    // crates/engine -> crates -> workspace root
    manifest_dir
        .ancestors()
        .nth(2)
        .unwrap_or(manifest_dir)
        .join("assets")
}

// ---------------------------------------------------------------------------
// GLB / glTF loading
// ---------------------------------------------------------------------------

/// A mesh primitive with its associated material, ready to add to a World.
pub struct LoadedMesh {
    /// Mesh geometry.
    pub mesh: Mesh,
    /// PBR material (scalar properties, texture indices into LoadedScene::textures).
    pub material: Material,
    /// Texture indices into [`LoadedScene::textures`] for this material.
    pub texture_indices: MaterialTextures,
    /// Whether both sides of triangles should be rendered.
    pub double_sided: bool,
}

/// Indices into [`LoadedScene::textures`] for a material's texture slots.
#[derive(Default)]
pub struct MaterialTextures {
    /// Albedo / base color texture index.
    pub albedo: Option<usize>,
    /// Normal map texture index.
    pub normal: Option<usize>,
    /// Roughness (G) + metallic (B) packed texture index.
    pub roughness_metallic: Option<usize>,
    /// Emissive texture index.
    pub emissive: Option<usize>,
}

/// A placed instance referencing a mesh in [`LoadedScene::meshes`].
pub struct LoadedInstance {
    /// Index into [`LoadedScene::meshes`].
    pub mesh_index: usize,
    /// World-space position.
    pub position: Vec3,
    /// Orientation.
    pub rotation: Quat,
    /// Scale.
    pub scale: Vec3,
}

/// The result of loading a glTF/GLB file.
pub struct LoadedScene {
    /// Unique mesh+material pairs (one per glTF primitive).
    pub meshes: Vec<LoadedMesh>,
    /// Texture image data referenced by materials.
    pub textures: Vec<TextureData>,
    /// Instances placed via the glTF node tree.
    pub instances: Vec<LoadedInstance>,
}

impl LoadedScene {
    /// Adds all meshes, materials, textures, and instances to the World.
    pub fn spawn(&self, world: &mut World) -> Vec<MeshInstanceKey> {
        let tex_keys: Vec<TextureKey> = self
            .textures
            .iter()
            .map(|td| {
                world.add_texture(TextureData {
                    pixels: td.pixels.clone(),
                    width: td.width,
                    height: td.height,
                })
            })
            .collect();

        let mesh_keys: Vec<_> = self
            .meshes
            .iter()
            .map(|lm| {
                let mesh_key = world.add_mesh(Mesh::new(
                    lm.mesh.vertices.clone(),
                    lm.mesh.indices.clone(),
                ));
                let mut mat = lm.material.clone();
                mat.albedo_map = lm.texture_indices.albedo.map(|i| tex_keys[i]);
                mat.normal_map = lm.texture_indices.normal.map(|i| tex_keys[i]);
                mat.roughness_metallic_map = lm.texture_indices.roughness_metallic.map(|i| tex_keys[i]);
                mat.emissive_map = lm.texture_indices.emissive.map(|i| tex_keys[i]);
                let mat_key = world.add_material(mat);
                (mesh_key, mat_key)
            })
            .collect();

        self.instances
            .iter()
            .map(|inst| {
                let (mesh_key, mat_key) = mesh_keys[inst.mesh_index];
                world.add_mesh_instance(MeshInstance {
                    mesh: mesh_key,
                    material: mat_key,
                    position: inst.position,
                    rotation: inst.rotation,
                    scale: inst.scale,
                    casts_shadows: true,
                    double_sided: self.meshes[inst.mesh_index].double_sided,
                })
            })
            .collect()
    }
}

/// Loads a glTF or GLB file, extracting mesh geometry and PBR material
/// properties. Returns an error string on failure.
pub fn load_glb(path: impl AsRef<Path>) -> Result<LoadedScene, String> {
    let path = path.as_ref();
    let (document, buffers, images) =
        gltf::import(path).map_err(|e| format!("failed to load {}: {e}", path.display()))?;

    // Convert glTF images to RGBA8 TextureData
    let mut textures: Vec<TextureData> = Vec::new();
    let mut image_index_map: HashMap<usize, usize> = HashMap::new();

    for (i, image) in images.iter().enumerate() {
        let rgba = match image.format {
            gltf::image::Format::R8G8B8A8 => image.pixels.clone(),
            gltf::image::Format::R8G8B8 => {
                let mut rgba = Vec::with_capacity(image.pixels.len() / 3 * 4);
                for chunk in image.pixels.chunks(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                rgba
            }
            other => {
                log::warn!("unsupported image format {other:?} for image {i}, skipping");
                continue;
            }
        };
        let tex_idx = textures.len();
        image_index_map.insert(i, tex_idx);
        textures.push(TextureData {
            pixels: rgba,
            width: image.width,
            height: image.height,
        });
    }

    let mut meshes = Vec::new();
    let mut mesh_index_map: HashMap<(usize, usize), usize> = HashMap::new();

    for gltf_mesh in document.meshes() {
        for primitive in gltf_mesh.primitives() {
            let key = (gltf_mesh.index(), primitive.index());
            if mesh_index_map.contains_key(&key) {
                continue;
            }

            let reader = primitive.reader(|buf| Some(&buffers[buf.index()]));

            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| format!("mesh {} has no positions", gltf_mesh.index()))?
                .collect();

            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|iter| iter.collect())
                .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()]);

            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|iter| iter.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);

            let tangents: Vec<[f32; 4]> = reader
                .read_tangents()
                .map(|iter| iter.collect())
                .unwrap_or_else(|| vec![[1.0, 0.0, 0.0, 1.0]; positions.len()]);

            let vertices: Vec<Vertex> = positions
                .iter()
                .enumerate()
                .map(|(i, &pos)| Vertex {
                    position: pos,
                    normal: normals[i],
                    uv: uvs[i],
                    tangent: tangents[i],
                })
                .collect();

            let indices: Vec<u32> = reader
                .read_indices()
                .map(|iter| iter.into_u32().collect())
                .unwrap_or_else(|| (0..vertices.len() as u32).collect());

            let gltf_mat = primitive.material();
            let material = extract_material(&primitive);
            let double_sided = gltf_mat.double_sided();
            let texture_indices = extract_texture_indices(&gltf_mat, &image_index_map);

            let idx = meshes.len();
            mesh_index_map.insert(key, idx);
            meshes.push(LoadedMesh {
                mesh: Mesh::new(vertices, indices),
                material,
                texture_indices,
                double_sided,
            });
        }
    }

    let mut instances = Vec::new();
    for scene in document.scenes() {
        for node in scene.nodes() {
            collect_instances(&node, Mat4::IDENTITY, &mesh_index_map, &mut instances);
        }
    }

    if instances.is_empty() && !meshes.is_empty() {
        instances.push(LoadedInstance {
            mesh_index: 0,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        });
    }

    log::info!(
        "loaded {}: {} meshes, {} textures, {} instances",
        path.display(),
        meshes.len(),
        textures.len(),
        instances.len()
    );

    Ok(LoadedScene {
        meshes,
        textures,
        instances,
    })
}

fn extract_texture_indices(
    mat: &gltf::Material<'_>,
    image_map: &HashMap<usize, usize>,
) -> MaterialTextures {
    let pbr = mat.pbr_metallic_roughness();
    MaterialTextures {
        albedo: pbr
            .base_color_texture()
            .and_then(|t| image_map.get(&t.texture().source().index()).copied()),
        normal: mat
            .normal_texture()
            .and_then(|t| image_map.get(&t.texture().source().index()).copied()),
        roughness_metallic: pbr
            .metallic_roughness_texture()
            .and_then(|t| image_map.get(&t.texture().source().index()).copied()),
        emissive: mat
            .emissive_texture()
            .and_then(|t| image_map.get(&t.texture().source().index()).copied()),
    }
}

fn extract_material(primitive: &gltf::Primitive<'_>) -> Material {
    let mat = primitive.material();
    let pbr = mat.pbr_metallic_roughness();
    let bc = pbr.base_color_factor();
    let em = mat.emissive_factor();
    let strength = mat.emissive_strength().unwrap_or(1.0);
    let emissive = Vec3::new(em[0] * strength, em[1] * strength, em[2] * strength);

    Material {
        base_color: Vec4::new(bc[0], bc[1], bc[2], bc[3]),
        roughness: pbr.roughness_factor(),
        metallic: pbr.metallic_factor(),
        emissive,
        ..Material::default()
    }
}

fn collect_instances(
    node: &gltf::Node<'_>,
    parent_transform: Mat4,
    mesh_map: &HashMap<(usize, usize), usize>,
    out: &mut Vec<LoadedInstance>,
) {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent_transform * local;

    if let Some(gltf_mesh) = node.mesh() {
        for primitive in gltf_mesh.primitives() {
            let key = (gltf_mesh.index(), primitive.index());
            if let Some(&mesh_index) = mesh_map.get(&key) {
                let (scale, rotation, translation) = world.to_scale_rotation_translation();
                out.push(LoadedInstance {
                    mesh_index,
                    position: translation,
                    rotation,
                    scale,
                });
            }
        }
    }

    for child in node.children() {
        collect_instances(&child, world, mesh_map, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<workspace root>`, the one directory these tests can rely on existing.
    fn workspace_root() -> PathBuf {
        dev_root()
            .parent()
            .expect("dev root has a parent")
            .to_path_buf()
    }

    #[test]
    fn override_wins_over_everything() {
        let forced = PathBuf::from("/somewhere/else");
        let resolved = resolve(Some(forced.clone()), Some(&workspace_root()));
        assert_eq!(resolved, forced);
    }

    #[test]
    fn exe_adjacent_assets_dir_is_used_when_present() {
        // The workspace root stands in for a shipped binary's directory: it is the one
        // place we know an `assets/` sibling exists.
        let resolved = resolve(None, Some(&workspace_root()));
        assert_eq!(resolved, dev_root());
        assert!(resolved.is_dir(), "assets/ is missing from the workspace");
    }

    #[test]
    fn falls_back_to_dev_root_without_an_exe_adjacent_dir() {
        // crates/engine has no assets/ sibling, so this exercises the fallback.
        assert_eq!(resolve(None, Some(Path::new(MANIFEST_DIR))), dev_root());
        assert_eq!(resolve(None, None), dev_root());
    }

    #[test]
    fn dev_root_points_at_the_workspace() {
        assert!(workspace_root().join("Cargo.toml").is_file());
        assert!(dev_root().ends_with("assets"));
    }

    #[test]
    fn relative_paths_join_component_wise() {
        let joined = path("textures/stone/albedo.png");
        assert!(joined.starts_with(root()));
        assert!(joined.ends_with(Path::new("textures").join("stone").join("albedo.png")));
    }

    #[test]
    fn empty_and_untidy_relative_paths_are_tolerated() {
        assert_eq!(path(""), root());
        assert_eq!(path("worlds//alpha"), path("worlds/alpha"));
    }
}
