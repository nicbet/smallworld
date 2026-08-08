//! Deferred lighting pass: light buffer, shadow atlas, clustered grid, shade.
//!
//! Reads the GBuffer textures produced by [`GBufferPass`](crate::gbuffer::GBufferPass)
//! and evaluates Cook-Torrance PBR lighting in a full-screen compute shader.
//! Outputs an HDR color texture blitted to the swapchain with Reinhard tone mapping.

use glam::{Mat4, Vec3, Vec4, Vec4Swizzles};

use crate::camera::FreeCamera;
use crate::cull::VisibilitySet;
use crate::gbuffer::GBuffer;
use crate::light::{Light, LightKind};
use crate::mesh::Vertex;
use crate::shaders;
use crate::stream::StreamOutput;
use crate::world::World;

// ---------------------------------------------------------------------------
// GPU-side structs (must match shade.wgsl / shadow.wgsl layout)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuLight {
    position_range: [f32; 4],
    direction_type: [f32; 4],
    color_intensity: [f32; 4],
    spot_params: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LightHeader {
    count: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadeUniforms {
    inv_view_proj: [f32; 16],
    camera_pos: [f32; 4],
    screen_size: [f32; 4],
    near_far: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowUniforms {
    light_view_proj: [f32; 16],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowViewGpu {
    view_proj: [f32; 16],
    viewport: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowHeader {
    count: u32,
    atlas_size: f32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ClusterParamsGpu {
    tiles_x: u32,
    tiles_y: u32,
    num_slices: u32,
    tile_size: u32,
    near: f32,
    log_ratio: f32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    model: [f32; 16],
    base_color: [f32; 4],
    roughness_metallic: [f32; 2],
    _pad: [f32; 2],
    emissive: [f32; 4],
}

const DRAW_UNIFORM_SIZE: u64 = size_of::<DrawUniforms>() as u64;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_LIGHTS: usize = 256;
const MAX_SHADOW_VIEWS: usize = 16;
const SHADOW_ATLAS_SIZE: u32 = 4096;
const TILE_SIZE: u32 = 80;
const NUM_DEPTH_SLICES: u32 = 24;
const MAX_LIGHTS_PER_CLUSTER: usize = 32;
const MAX_DRAWS: u32 = 256;

// ---------------------------------------------------------------------------
// LightBuffer
// ---------------------------------------------------------------------------

pub(crate) struct LightBuffer {
    buffer: wgpu::Buffer,
    header_buffer: wgpu::Buffer,
    count: u32,
}

impl LightBuffer {
    fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light_buffer"),
            size: (MAX_LIGHTS * size_of::<GpuLight>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let header_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("light_header"),
            size: size_of::<LightHeader>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            header_buffer,
            count: 0,
        }
    }

    fn upload(&mut self, queue: &wgpu::Queue, world: &World, visibility: &VisibilitySet) -> Vec<PackedLight> {
        let mut gpu_lights = Vec::with_capacity(visibility.lights.len().min(MAX_LIGHTS));
        let mut packed = Vec::with_capacity(gpu_lights.capacity());

        for &key in &visibility.lights {
            if gpu_lights.len() >= MAX_LIGHTS {
                log::warn!("light count exceeds {MAX_LIGHTS}, clamping");
                break;
            }
            let Some(light) = world.light(key) else {
                continue;
            };
            let (gpu, info) = pack_light(light, -1);
            packed.push(info);
            gpu_lights.push(gpu);
        }

        self.count = gpu_lights.len() as u32;

        let header = LightHeader {
            count: self.count,
            _pad: [0; 3],
        };
        queue.write_buffer(&self.header_buffer, 0, bytemuck::bytes_of(&header));

        if !gpu_lights.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&gpu_lights));
        }

        packed
    }
}

struct PackedLight {
    kind: PackedLightKind,
    position: Vec3,
    range: f32,
    casts_shadows: bool,
    direction: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PackedLightKind {
    Directional,
    Point,
    Spot,
}

fn pack_light(light: &Light, shadow_index: i32) -> (GpuLight, PackedLight) {
    let (pos, range, dir, light_type, inner_cos, outer_cos) = match &light.kind {
        LightKind::Directional { direction } => {
            (Vec3::ZERO, -1.0_f32, *direction, 0u32, 0.0_f32, 0.0_f32)
        }
        LightKind::Point { position, range } => {
            (*position, *range, Vec3::ZERO, 1u32, 0.0_f32, 0.0_f32)
        }
        LightKind::Spot {
            position,
            direction,
            range,
            inner_angle,
            outer_angle,
        } => (
            *position,
            *range,
            *direction,
            2u32,
            inner_angle.cos(),
            outer_angle.cos(),
        ),
    };

    let gpu = GpuLight {
        position_range: [pos.x, pos.y, pos.z, range],
        direction_type: [dir.x, dir.y, dir.z, light_type as f32],
        color_intensity: [light.color.x, light.color.y, light.color.z, light.intensity],
        spot_params: [inner_cos, outer_cos, shadow_index as f32, 0.0],
    };

    let info = PackedLight {
        kind: match light_type {
            0 => PackedLightKind::Directional,
            1 => PackedLightKind::Point,
            _ => PackedLightKind::Spot,
        },
        position: pos,
        range,
        casts_shadows: light.casts_shadows,
        direction: dir,
    };

    (gpu, info)
}

// ---------------------------------------------------------------------------
// ShadowAtlas
// ---------------------------------------------------------------------------

struct ShadowRegion {
    x: u32,
    y: u32,
    size: u32,
}

pub(crate) struct ShadowAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    views_buffer: wgpu::Buffer,
    header_buffer: wgpu::Buffer,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_uniform_buf: wgpu::Buffer,
    shadow_frame_bind_group: wgpu::BindGroup,
    draw_uniform_buf: wgpu::Buffer,
    draw_bind_group: wgpu::BindGroup,
    max_draws: u32,
}

impl ShadowAtlas {
    fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow_atlas"),
            size: wgpu::Extent3d {
                width: SHADOW_ATLAS_SIZE,
                height: SHADOW_ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let views_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_views"),
            size: (MAX_SHADOW_VIEWS * size_of::<ShadowViewGpu>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let header_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_header"),
            size: size_of::<ShadowHeader>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Shadow pipeline
        let shadow_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow"),
            source: wgpu::ShaderSource::Wgsl(shaders::load(shaders::Shader::Shadow)),
        });

        let shadow_frame_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shadow_frame"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow_draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let shadow_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_uniforms"),
            size: size_of::<ShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let min_align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let draw_stride = DRAW_UNIFORM_SIZE.div_ceil(min_align) * min_align;
        let draw_buf_size = draw_stride * MAX_DRAWS as u64;

        let draw_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_draw_uniforms"),
            size: draw_buf_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shadow_frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow_frame"),
            layout: &shadow_frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_uniform_buf.as_entire_binding(),
            }],
        });

        let draw_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow_draw"),
            layout: &draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &draw_uniform_buf,
                    offset: 0,
                    size: Some(wgpu::BufferSize::new(DRAW_UNIFORM_SIZE).unwrap()),
                }),
            }],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3,
                1 => Float32x3,
                2 => Float32x2,
                3 => Float32x4,
            ],
        };

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(&shadow_frame_layout), Some(&draw_layout)],
            immediate_size: 0,
        });

        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_module,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout)],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            texture,
            view,
            views_buffer,
            header_buffer,
            shadow_pipeline,
            shadow_uniform_buf,
            shadow_frame_bind_group,
            draw_uniform_buf,
            draw_bind_group,
            max_draws: MAX_DRAWS,
        }
    }

    fn allocate_regions(&self, count: usize) -> Vec<ShadowRegion> {
        if count == 0 {
            return Vec::new();
        }
        let subdivisions = (count as f32).sqrt().ceil() as u32;
        let region_size = SHADOW_ATLAS_SIZE / subdivisions;
        let mut regions = Vec::with_capacity(count);
        for i in 0..count as u32 {
            let gx = i % subdivisions;
            let gy = i / subdivisions;
            regions.push(ShadowRegion {
                x: gx * region_size,
                y: gy * region_size,
                size: region_size,
            });
        }
        regions
    }

    #[allow(clippy::too_many_arguments)]
    fn render_shadow_pass(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        light_view_proj: Mat4,
        region: &ShadowRegion,
        world: &World,
        stream_output: &StreamOutput<'_>,
    ) {
        let uniforms = ShadowUniforms {
            light_view_proj: light_view_proj.to_cols_array(),
        };
        queue.write_buffer(&self.shadow_uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let min_align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let draw_stride = DRAW_UNIFORM_SIZE.div_ceil(min_align) * min_align;

        let mut draw_index: u32 = 0;
        let mut draw_offsets: Vec<u32> = Vec::new();

        // Volume meshes
        for (_key, _gpu_mesh) in &stream_output.volume_meshes {
            if draw_index >= self.max_draws {
                break;
            }
            let du = DrawUniforms {
                model: Mat4::IDENTITY.to_cols_array(),
                base_color: [0.0; 4],
                roughness_metallic: [0.0; 2],
                _pad: [0.0; 2],
                emissive: [0.0; 4],
            };
            let offset = draw_index as u64 * draw_stride;
            queue.write_buffer(&self.draw_uniform_buf, offset, bytemuck::bytes_of(&du));
            draw_offsets.push(offset as u32);
            draw_index += 1;
        }

        // Mesh instances (only shadow casters)
        for (key, _gpu_mesh) in &stream_output.mesh_instances {
            if draw_index >= self.max_draws {
                break;
            }
            let model = if let Some(inst) = world.mesh_instance(*key) {
                if !inst.casts_shadows {
                    continue;
                }
                Mat4::from_scale_rotation_translation(inst.scale, inst.rotation, inst.position)
            } else {
                continue;
            };
            let du = DrawUniforms {
                model: model.to_cols_array(),
                base_color: [0.0; 4],
                roughness_metallic: [0.0; 2],
                _pad: [0.0; 2],
                emissive: [0.0; 4],
            };
            let offset = draw_index as u64 * draw_stride;
            queue.write_buffer(&self.draw_uniform_buf, offset, bytemuck::bytes_of(&du));
            draw_offsets.push(offset as u32);
            draw_index += 1;
        }

        let region_view = self.texture.create_view(&wgpu::TextureViewDescriptor::default());

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &region_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            rpass.set_viewport(
                region.x as f32,
                region.y as f32,
                region.size as f32,
                region.size as f32,
                0.0,
                1.0,
            );
            rpass.set_scissor_rect(region.x, region.y, region.size, region.size);
            rpass.set_pipeline(&self.shadow_pipeline);
            rpass.set_bind_group(0, &self.shadow_frame_bind_group, &[]);

            let mut mesh_idx = 0;
            for (_key, gpu_mesh) in &stream_output.volume_meshes {
                if mesh_idx >= draw_offsets.len() {
                    break;
                }
                rpass.set_bind_group(1, &self.draw_bind_group, &[draw_offsets[mesh_idx]]);
                rpass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                rpass.set_index_buffer(
                    gpu_mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                rpass.draw_indexed(0..gpu_mesh.index_count, 0, 0..1);
                mesh_idx += 1;
            }

            for (key, gpu_mesh) in &stream_output.mesh_instances {
                if mesh_idx >= draw_offsets.len() {
                    break;
                }
                if let Some(inst) = world.mesh_instance(*key)
                    && !inst.casts_shadows
                {
                    continue;
                }
                rpass.set_bind_group(1, &self.draw_bind_group, &[draw_offsets[mesh_idx]]);
                rpass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                rpass.set_index_buffer(
                    gpu_mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                rpass.draw_indexed(0..gpu_mesh.index_count, 0, 0..1);
                mesh_idx += 1;
            }
        }
    }
}

fn compute_directional_shadow_vp(direction: Vec3, camera: &FreeCamera) -> Mat4 {
    let center = camera.position + camera.forward() * 10.0;
    let half_extent = 15.0_f32;
    let depth_range = 60.0_f32;
    let light_dir = direction.normalize();
    let light_pos = center - light_dir * (depth_range * 0.5);
    let light_view = glam::camera::rh::view::look_at_mat4(light_pos, center, Vec3::Y);
    let light_proj = glam::camera::rh::proj::directx::orthographic(
        -half_extent,
        half_extent,
        -half_extent,
        half_extent,
        0.1,
        depth_range,
    );
    light_proj * light_view
}

// ---------------------------------------------------------------------------
// ClusteredLightGrid
// ---------------------------------------------------------------------------

pub(crate) struct ClusteredLightGrid {
    offsets_buffer: wgpu::Buffer,
    counts_buffer: wgpu::Buffer,
    indices_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    tiles_x: u32,
    tiles_y: u32,
}

impl ClusteredLightGrid {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let tiles_x = width.div_ceil(TILE_SIZE);
        let tiles_y = height.div_ceil(TILE_SIZE);
        let num_clusters = (tiles_x * tiles_y * NUM_DEPTH_SLICES) as usize;

        let offsets_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cluster_offsets"),
            size: (num_clusters * size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let counts_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cluster_counts"),
            size: (num_clusters * size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let max_indices = num_clusters * MAX_LIGHTS_PER_CLUSTER;
        let indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cluster_light_indices"),
            size: (max_indices * size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cluster_params"),
            size: size_of::<ClusterParamsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            offsets_buffer,
            counts_buffer,
            indices_buffer,
            params_buffer,
            tiles_x,
            tiles_y,
        }
    }

    fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let new_tx = width.div_ceil(TILE_SIZE);
        let new_ty = height.div_ceil(TILE_SIZE);
        if new_tx == self.tiles_x && new_ty == self.tiles_y {
            return;
        }
        *self = Self::new(device, width, height);
    }

    fn assign(
        &self,
        queue: &wgpu::Queue,
        packed_lights: &[PackedLight],
        camera: &FreeCamera,
        width: u32,
        height: u32,
    ) {
        let tiles_x = self.tiles_x as usize;
        let tiles_y = self.tiles_y as usize;
        let num_slices = NUM_DEPTH_SLICES as usize;
        let num_clusters = tiles_x * tiles_y * num_slices;

        let cluster_near = camera.near.max(0.5);
        let log_ratio = num_slices as f32 / (camera.far / cluster_near).ln();

        let params = ClusterParamsGpu {
            tiles_x: tiles_x as u32,
            tiles_y: tiles_y as u32,
            num_slices: num_slices as u32,
            tile_size: TILE_SIZE,
            near: cluster_near,
            log_ratio,
            _pad: [0; 2],
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        let mut counts = vec![0u32; num_clusters];
        let mut indices_lists: Vec<Vec<u32>> = vec![Vec::new(); num_clusters];

        let view = camera.view_matrix();
        let proj = camera.projection_matrix();
        let view_proj = proj * view;

        for (light_idx, light) in packed_lights.iter().enumerate() {
            let light_idx = light_idx as u32;

            if light.kind == PackedLightKind::Directional {
                for cluster in 0..num_clusters {
                    if counts[cluster] < MAX_LIGHTS_PER_CLUSTER as u32 {
                        indices_lists[cluster].push(light_idx);
                        counts[cluster] += 1;
                    }
                }
                continue;
            }

            let (min_tile, max_tile, min_slice, max_slice) =
                light_cluster_bounds(light, &view_proj, tiles_x, tiles_y, num_slices,
                                     width, height, cluster_near, camera.far);

            for sz in min_slice..=max_slice {
                for ty in min_tile.1..=max_tile.1 {
                    for tx in min_tile.0..=max_tile.0 {
                        let idx = tx + tiles_x * (ty + tiles_y * sz);
                        if idx < num_clusters && counts[idx] < MAX_LIGHTS_PER_CLUSTER as u32 {
                            indices_lists[idx].push(light_idx);
                            counts[idx] += 1;
                        }
                    }
                }
            }
        }

        // Pack into flat arrays
        let mut offsets = vec![0u32; num_clusters];
        let mut flat_indices: Vec<u32> = Vec::new();
        for i in 0..num_clusters {
            offsets[i] = flat_indices.len() as u32;
            flat_indices.extend_from_slice(&indices_lists[i]);
        }

        queue.write_buffer(&self.offsets_buffer, 0, bytemuck::cast_slice(&offsets));
        queue.write_buffer(&self.counts_buffer, 0, bytemuck::cast_slice(&counts));
        if !flat_indices.is_empty() {
            let max_size = self.indices_buffer.size() as usize / size_of::<u32>();
            let write_len = flat_indices.len().min(max_size);
            queue.write_buffer(
                &self.indices_buffer,
                0,
                bytemuck::cast_slice(&flat_indices[..write_len]),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn light_cluster_bounds(
    light: &PackedLight,
    view_proj: &Mat4,
    tiles_x: usize,
    tiles_y: usize,
    num_slices: usize,
    width: u32,
    height: u32,
    cluster_near: f32,
    far: f32,
) -> ((usize, usize), (usize, usize), usize, usize) {
    let range = light.range.max(0.1);
    let center = light.position;

    let corners = [
        center + Vec3::new(range, range, range),
        center + Vec3::new(-range, range, range),
        center + Vec3::new(range, -range, range),
        center + Vec3::new(range, range, -range),
        center + Vec3::new(-range, -range, range),
        center + Vec3::new(-range, range, -range),
        center + Vec3::new(range, -range, -range),
        center + Vec3::new(-range, -range, -range),
    ];

    let mut min_px = width as f32;
    let mut max_px = 0.0_f32;
    let mut min_py = height as f32;
    let mut max_py = 0.0_f32;
    let mut min_depth = far;
    let mut max_depth = cluster_near;

    for corner in &corners {
        let clip = *view_proj * Vec4::new(corner.x, corner.y, corner.z, 1.0);
        if clip.w <= 0.0 {
            return ((0, 0), (tiles_x.saturating_sub(1), tiles_y.saturating_sub(1)),
                    0, num_slices.saturating_sub(1));
        }
        let ndc = clip.xyz() / clip.w;
        let px = (ndc.x * 0.5 + 0.5) * width as f32;
        let py = (0.5 - ndc.y * 0.5) * height as f32;
        let linear_z = cluster_near.max(clip.w);

        min_px = min_px.min(px);
        max_px = max_px.max(px);
        min_py = min_py.min(py);
        max_py = max_py.max(py);
        min_depth = min_depth.min(linear_z);
        max_depth = max_depth.max(linear_z);
    }

    let tile_size = TILE_SIZE as f32;
    let min_tx = (min_px / tile_size).floor().max(0.0) as usize;
    let max_tx = (max_px / tile_size).floor().min(tiles_x as f32 - 1.0).max(0.0) as usize;
    let min_ty = (min_py / tile_size).floor().max(0.0) as usize;
    let max_ty = (max_py / tile_size).floor().min(tiles_y as f32 - 1.0).max(0.0) as usize;

    let log_ratio = num_slices as f32 / (far / cluster_near).ln();
    let min_sz = ((min_depth / cluster_near).ln() * log_ratio)
        .floor()
        .max(0.0) as usize;
    let max_sz = ((max_depth / cluster_near).ln() * log_ratio)
        .floor()
        .min(num_slices as f32 - 1.0)
        .max(0.0) as usize;

    ((min_tx, min_ty), (max_tx, max_ty), min_sz, max_sz)
}

// ---------------------------------------------------------------------------
// LightingPass
// ---------------------------------------------------------------------------

/// Deferred lighting pass. Owns shadow atlas, light buffer, clustered grid,
/// shade compute pipeline, HDR output, and the Reinhard blit to swapchain.
pub struct LightingPass {
    light_buffer: LightBuffer,
    shadow_atlas: ShadowAtlas,
    cluster_grid: ClusteredLightGrid,
    shade_pipeline: wgpu::ComputePipeline,
    shade_gbuf_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    shade_light_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    shade_shadow_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    shade_cluster_layout: wgpu::BindGroupLayout,
    shade_uniform_buf: wgpu::Buffer,
    shade_light_bind_group: wgpu::BindGroup,
    shade_shadow_bind_group: wgpu::BindGroup,
    shade_cluster_bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    shadow_sampler: wgpu::Sampler,
    #[allow(dead_code)]
    hdr_texture: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

impl LightingPass {
    /// Creates the lighting pass for the given surface format and size.
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let light_buffer = LightBuffer::new(device);
        let shadow_atlas = ShadowAtlas::new(device);
        let cluster_grid = ClusteredLightGrid::new(device, width, height);

        // HDR output texture
        let (hdr_texture, hdr_view) = create_hdr_texture(device, width, height);

        // Shadow sampler (comparison)
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow_compare"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Shade compute pipeline
        let shade_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shade"),
            source: wgpu::ShaderSource::Wgsl(shaders::load(shaders::Shader::Shade)),
        });

        // Group 0: GBuffer + HDR output
        let shade_gbuf_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shade_gbuf"),
                entries: &[
                    bgl_texture(0, wgpu::TextureSampleType::Depth),
                    bgl_texture(1, wgpu::TextureSampleType::Float { filterable: false }),
                    bgl_texture(2, wgpu::TextureSampleType::Float { filterable: false }),
                    bgl_texture(3, wgpu::TextureSampleType::Float { filterable: false }),
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba16Float,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    bgl_texture(5, wgpu::TextureSampleType::Float { filterable: false }),
                ],
            });

        // Group 1: Camera + lights
        let shade_light_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shade_lights"),
                entries: &[
                    bgl_uniform(0),
                    bgl_uniform(1),
                    bgl_storage_ro(2),
                ],
            });

        // Group 2: Shadow atlas
        let shade_shadow_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shade_shadow"),
                entries: &[
                    bgl_texture(0, wgpu::TextureSampleType::Depth),
                    bgl_uniform(1),
                    bgl_storage_ro(2),
                ],
            });

        // Group 3: Cluster grid
        let shade_cluster_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shade_cluster"),
                entries: &[
                    bgl_uniform(0),
                    bgl_storage_ro(1),
                    bgl_storage_ro(2),
                    bgl_storage_ro(3),
                ],
            });

        let shade_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shade"),
                bind_group_layouts: &[
                    Some(&shade_gbuf_layout),
                    Some(&shade_light_layout),
                    Some(&shade_shadow_layout),
                    Some(&shade_cluster_layout),
                ],
                immediate_size: 0,
            });

        let shade_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("shade"),
            layout: Some(&shade_pipeline_layout),
            module: &shade_module,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Shade uniforms buffer
        let shade_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shade_uniforms"),
            size: size_of::<ShadeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Static bind groups for groups 1, 2, 3
        let shade_light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shade_lights"),
            layout: &shade_light_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: shade_uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.header_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: light_buffer.buffer.as_entire_binding(),
                },
            ],
        });

        let shade_shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shade_shadow"),
            layout: &shade_shadow_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_atlas.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: shadow_atlas.header_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shadow_atlas.views_buffer.as_entire_binding(),
                },
            ],
        });

        let shade_cluster_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shade_cluster"),
            layout: &shade_cluster_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: cluster_grid.params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cluster_grid.offsets_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: cluster_grid.counts_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: cluster_grid.indices_buffer.as_entire_binding(),
                },
            ],
        });

        // Blit pipeline (HDR → surface with Reinhard)
        let blit_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hdr_blit"),
            source: wgpu::ShaderSource::Wgsl(shaders::load(shaders::Shader::Blit)),
        });

        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("hdr_blit"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hdr_blit"),
            bind_group_layouts: &[Some(&blit_bind_group_layout)],
            immediate_size: 0,
        });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hdr_blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hdr_blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            light_buffer,
            shadow_atlas,
            cluster_grid,
            shade_pipeline,
            shade_gbuf_layout,
            shade_light_layout,
            shade_shadow_layout,
            shade_cluster_layout,
            shade_uniform_buf,
            shade_light_bind_group,
            shade_shadow_bind_group,
            shade_cluster_bind_group,
            shadow_sampler,
            hdr_texture,
            hdr_view,
            blit_pipeline,
            blit_bind_group_layout,
            blit_sampler,
            width,
            height,
        }
    }

    /// Recreates HDR texture and cluster grid on resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        let (tex, view) = create_hdr_texture(device, width, height);
        self.hdr_texture = tex;
        self.hdr_view = view;
        self.cluster_grid.resize(device, width, height);
    }

    /// Renders shadows, evaluates deferred lighting, and blits HDR to surface.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        gbuffer: &GBuffer,
        camera: &FreeCamera,
        world: &World,
        visibility: &VisibilitySet,
        stream_output: &StreamOutput<'_>,
    ) {
        // 1. Upload lights
        let packed_lights = self.light_buffer.upload(queue, world, visibility);

        // 2. Shadow pass — find first shadow-casting directional light
        let mut shadow_view_count = 0u32;
        let mut shadow_views_gpu: Vec<ShadowViewGpu> = Vec::new();

        // Re-upload lights with shadow indices
        let mut shadow_caster_indices: Vec<(usize, Mat4, ShadowRegion)> = Vec::new();
        for (i, pl) in packed_lights.iter().enumerate() {
            if pl.casts_shadows && pl.kind == PackedLightKind::Directional {
                let vp = compute_directional_shadow_vp(pl.direction, camera);
                shadow_caster_indices.push((i, vp, ShadowRegion { x: 0, y: 0, size: 0 }));
            }
        }

        if !shadow_caster_indices.is_empty() {
            let regions = self.shadow_atlas.allocate_regions(shadow_caster_indices.len());
            for (idx, (_, _, region)) in shadow_caster_indices.iter_mut().enumerate() {
                if idx < regions.len() {
                    *region = ShadowRegion {
                        x: regions[idx].x,
                        y: regions[idx].y,
                        size: regions[idx].size,
                    };
                }
            }

            // Re-pack lights with shadow indices
            let mut gpu_lights: Vec<GpuLight> = Vec::new();
            let mut shadow_map: std::collections::HashMap<usize, i32> = std::collections::HashMap::new();
            for (si, (light_i, _, _)) in shadow_caster_indices.iter().enumerate() {
                shadow_map.insert(*light_i, si as i32);
            }

            for (i, key) in visibility.lights.iter().enumerate() {
                if i >= MAX_LIGHTS {
                    break;
                }
                let Some(light) = world.light(*key) else { continue };
                let shadow_idx = shadow_map.get(&i).copied().unwrap_or(-1);
                let (gpu, _) = pack_light(light, shadow_idx);
                gpu_lights.push(gpu);
            }
            if !gpu_lights.is_empty() {
                queue.write_buffer(
                    &self.light_buffer.buffer,
                    0,
                    bytemuck::cast_slice(&gpu_lights),
                );
            }

            // Render shadow maps and build view array
            for (_, vp, region) in &shadow_caster_indices {
                self.shadow_atlas.render_shadow_pass(
                    device, queue, encoder, *vp, region, world, stream_output,
                );
                shadow_views_gpu.push(ShadowViewGpu {
                    view_proj: vp.to_cols_array(),
                    viewport: [region.x as f32, region.y as f32, region.size as f32, region.size as f32],
                });
                shadow_view_count += 1;
            }
        }

        // Upload shadow views
        let shadow_header = ShadowHeader {
            count: shadow_view_count,
            atlas_size: SHADOW_ATLAS_SIZE as f32,
            _pad: [0; 2],
        };
        queue.write_buffer(
            &self.shadow_atlas.header_buffer,
            0,
            bytemuck::bytes_of(&shadow_header),
        );
        if !shadow_views_gpu.is_empty() {
            queue.write_buffer(
                &self.shadow_atlas.views_buffer,
                0,
                bytemuck::cast_slice(&shadow_views_gpu),
            );
        }

        // 3. Cluster assignment
        self.cluster_grid.assign(queue, &packed_lights, camera, self.width, self.height);

        // 4. Shade uniforms
        let view = camera.view_matrix();
        let proj = camera.projection_matrix();
        let view_proj = proj * view;
        let inv_view_proj = view_proj.inverse();

        let shade_uniforms = ShadeUniforms {
            inv_view_proj: inv_view_proj.to_cols_array(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 0.0],
            screen_size: [
                self.width as f32,
                self.height as f32,
                1.0 / self.width as f32,
                1.0 / self.height as f32,
            ],
            near_far: [camera.near, camera.far, 0.0, 0.0],
        };
        queue.write_buffer(&self.shade_uniform_buf, 0, bytemuck::bytes_of(&shade_uniforms));

        // 5. Shade compute dispatch
        let shade_gbuf_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shade_gbuf"),
            layout: &self.shade_gbuf_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.material_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&gbuffer.emissive_view),
                },
            ],
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("shade"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.shade_pipeline);
            cpass.set_bind_group(0, &shade_gbuf_bind_group, &[]);
            cpass.set_bind_group(1, &self.shade_light_bind_group, &[]);
            cpass.set_bind_group(2, &self.shade_shadow_bind_group, &[]);
            cpass.set_bind_group(3, &self.shade_cluster_bind_group, &[]);
            cpass.dispatch_workgroups(
                self.width.div_ceil(8),
                self.height.div_ceil(8),
                1,
            );
        }

        // 6. Blit HDR → surface with Reinhard
        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hdr_blit"),
            layout: &self.blit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blit_sampler),
                },
            ],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hdr_blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(&self.blit_pipeline);
            rpass.set_bind_group(0, &blit_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn create_hdr_texture(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hdr_color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn bgl_texture(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn bgl_uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bgl_storage_ro(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::Light;

    #[test]
    fn pack_directional_light() {
        let light = Light::directional(Vec3::new(0.0, -1.0, 0.0), Vec3::ONE, 3.0);
        let (gpu, info) = pack_light(&light, 0);

        assert_eq!(info.kind, PackedLightKind::Directional);
        assert_eq!(gpu.direction_type[3], 0.0); // type = directional
        assert_eq!(gpu.position_range[3], -1.0); // range sentinel
        assert_eq!(gpu.color_intensity[3], 3.0); // intensity
        assert_eq!(gpu.spot_params[2], 0.0); // shadow index
    }

    #[test]
    fn pack_point_light() {
        let light = Light::point(Vec3::new(1.0, 2.0, 3.0), 15.0, Vec3::ONE, 8.0);
        let (gpu, info) = pack_light(&light, -1);

        assert_eq!(info.kind, PackedLightKind::Point);
        assert_eq!(gpu.direction_type[3], 1.0);
        assert_eq!(gpu.position_range, [1.0, 2.0, 3.0, 15.0]);
        assert_eq!(gpu.color_intensity[3], 8.0);
        assert_eq!(gpu.spot_params[2], -1.0); // no shadow
    }

    #[test]
    fn pack_spot_light() {
        let light = Light::spot(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            20.0,
            0.3,
            0.5,
            Vec3::ONE,
            10.0,
        );
        let (gpu, info) = pack_light(&light, 2);

        assert_eq!(info.kind, PackedLightKind::Spot);
        assert_eq!(gpu.direction_type[3], 2.0);
        assert!((gpu.spot_params[0] - 0.3_f32.cos()).abs() < 1e-5);
        assert!((gpu.spot_params[1] - 0.5_f32.cos()).abs() < 1e-5);
        assert_eq!(gpu.spot_params[2], 2.0); // shadow index
    }

    #[test]
    fn shadow_atlas_allocates_single_region() {
        let instance = crate::gpu::GpuContext::create_instance();
        let ctx = pollster::block_on(crate::gpu::GpuContext::headless(instance));
        let atlas = ShadowAtlas::new(&ctx.device);

        let regions = atlas.allocate_regions(1);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].x, 0);
        assert_eq!(regions[0].y, 0);
        assert_eq!(regions[0].size, SHADOW_ATLAS_SIZE);
    }

    #[test]
    fn shadow_atlas_subdivides_for_multiple() {
        let instance = crate::gpu::GpuContext::create_instance();
        let ctx = pollster::block_on(crate::gpu::GpuContext::headless(instance));
        let atlas = ShadowAtlas::new(&ctx.device);

        let regions = atlas.allocate_regions(4);
        assert_eq!(regions.len(), 4);
        let half = SHADOW_ATLAS_SIZE / 2;
        assert_eq!(regions[0].size, half);
        assert_eq!(regions[1].x, half);
        assert_eq!(regions[2].y, half);
        assert_eq!(regions[3].x, half);
        assert_eq!(regions[3].y, half);
    }

    #[test]
    fn cluster_grid_dimensions() {
        let instance = crate::gpu::GpuContext::create_instance();
        let ctx = pollster::block_on(crate::gpu::GpuContext::headless(instance));
        let grid = ClusteredLightGrid::new(&ctx.device, 1280, 720);

        assert_eq!(grid.tiles_x, 16);
        assert_eq!(grid.tiles_y, 9);
    }

    #[test]
    fn cluster_grid_resize_changes_tiles() {
        let instance = crate::gpu::GpuContext::create_instance();
        let ctx = pollster::block_on(crate::gpu::GpuContext::headless(instance));
        let mut grid = ClusteredLightGrid::new(&ctx.device, 1280, 720);
        assert_eq!(grid.tiles_x, 16);

        grid.resize(&ctx.device, 1920, 1080);
        assert_eq!(grid.tiles_x, 24);
        assert_eq!(grid.tiles_y, 14);
    }

    #[test]
    fn max_lights_constant() {
        assert_eq!(MAX_LIGHTS, 256);
        assert!(size_of::<GpuLight>() == 64);
    }
}
