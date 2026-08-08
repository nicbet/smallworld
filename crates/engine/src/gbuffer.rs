//! GBuffer pass: first sub-stages of Execute.
//!
//! Renders visible cached meshes into a GBuffer (albedo, normal,
//! material) with depth. Builds the HZB mip chain from depth for
//! next frame's occlusion culling.

use std::collections::HashMap;

use glam::Mat4;

use crate::camera::FreeCamera;
use crate::mesh::Vertex;
use crate::shaders;
use crate::stream::{GpuMesh, StreamOutput};
use crate::world::{TextureKey, World};

// ---------------------------------------------------------------------------
// Uniform structs (must match WGSL layout)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniforms {
    view_proj: [f32; 16],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    model: [f32; 16],
    base_color: [f32; 4],
    roughness_metallic: [f32; 2],
    _pad: [f32; 2],
}

const DRAW_UNIFORM_SIZE: u64 = size_of::<DrawUniforms>() as u64;

// ---------------------------------------------------------------------------
// GBuffer textures
// ---------------------------------------------------------------------------

/// The GBuffer render targets. Recreated on resize.
pub struct GBuffer {
    /// Depth render target view.
    pub depth_view: wgpu::TextureView,
    /// Albedo (base color) render target view.
    pub albedo_view: wgpu::TextureView,
    /// Octahedral-encoded normal render target view.
    pub normal_view: wgpu::TextureView,
    /// Material properties (roughness, metallic) render target view.
    pub material_view: wgpu::TextureView,
    #[allow(dead_code)]
    albedo_tex: wgpu::Texture,
    #[allow(dead_code)]
    depth_tex: wgpu::Texture,
    width: u32,
    height: u32,
}

impl GBuffer {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gbuf_depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let albedo_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gbuf_albedo"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let normal_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gbuf_normal"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let material_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gbuf_material"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let default_view = wgpu::TextureViewDescriptor::default();

        Self {
            depth_view: depth_tex.create_view(&default_view),
            albedo_view: albedo_tex.create_view(&default_view),
            normal_view: normal_tex.create_view(&default_view),
            material_view: material_tex.create_view(&default_view),
            albedo_tex,
            depth_tex,
            width,
            height,
        }
    }
}

// ---------------------------------------------------------------------------
// HZB builder
// ---------------------------------------------------------------------------

struct HzbBuilder {
    texture: Option<wgpu::Texture>,
    mip_count: u32,
}

impl HzbBuilder {
    fn new() -> Self {
        Self {
            texture: None,
            mip_count: 0,
        }
    }

    fn ensure_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let mip_count = (width.max(height) as f32).log2().floor() as u32 + 1;
        if self.texture.is_some() && self.mip_count == mip_count {
            return;
        }
        self.mip_count = mip_count;
        self.texture = Some(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hzb"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
    }

    #[allow(dead_code)]
    fn view(&self) -> Option<wgpu::TextureView> {
        self.texture
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
    }
}

// ---------------------------------------------------------------------------
// GPU texture cache
// ---------------------------------------------------------------------------

struct GpuTextureEntry {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct TextureCache {
    cache: HashMap<TextureKey, GpuTextureEntry>,
    fallback_albedo: GpuTextureEntry,
    fallback_normal: GpuTextureEntry,
    fallback_roughness_metallic: GpuTextureEntry,
}

impl TextureCache {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let fallback_albedo = create_1x1_texture(device, queue, "fallback_albedo", &[255, 255, 255, 255]);
        let fallback_normal = create_1x1_texture(device, queue, "fallback_normal", &[128, 128, 255, 255]);
        let fallback_roughness_metallic =
            create_1x1_texture(device, queue, "fallback_rm", &[255, 128, 0, 255]);

        Self {
            cache: HashMap::new(),
            fallback_albedo,
            fallback_normal,
            fallback_roughness_metallic,
        }
    }

    fn get_or_upload(
        &mut self,
        key: TextureKey,
        world: &World,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> &wgpu::TextureView {
        if let std::collections::hash_map::Entry::Vacant(e) = self.cache.entry(key)
            && let Some(data) = world.texture(key)
        {
            e.insert(upload_texture(device, queue, data));
        }
        self.cache
            .get(&key)
            .map(|e| &e.view)
            .unwrap_or(&self.fallback_albedo.view)
    }

    fn view_or_fallback<'a>(&'a self, key: Option<TextureKey>, fallback: &'a wgpu::TextureView) -> &'a wgpu::TextureView {
        key.and_then(|k| self.cache.get(&k))
            .map(|e| &e.view)
            .unwrap_or(fallback)
    }
}

fn create_1x1_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    rgba: &[u8; 4],
) -> GpuTextureEntry {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuTextureEntry { texture, view }
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: &crate::texture::TextureData,
) -> GpuTextureEntry {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("material_texture"),
        size: wgpu::Extent3d {
            width: data.width,
            height: data.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * data.width),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: data.width,
            height: data.height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuTextureEntry { texture, view }
}

// ---------------------------------------------------------------------------
// GBufferPass
// ---------------------------------------------------------------------------

/// Renders meshes into the GBuffer and builds HZB.
pub struct GBufferPass {
    gbuffer: GBuffer,
    gbuffer_pipeline: wgpu::RenderPipeline,
    gbuffer_pipeline_double_sided: wgpu::RenderPipeline,
    #[allow(dead_code)]
    frame_bind_group_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    draw_bind_group_layout: wgpu::BindGroupLayout,
    tex_bind_group_layout: wgpu::BindGroupLayout,
    frame_uniform_buf: wgpu::Buffer,
    draw_uniform_buf: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    draw_bind_group: wgpu::BindGroup,
    tex_sampler: wgpu::Sampler,
    texture_cache: TextureCache,
    hzb: HzbBuilder,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
    max_draws: u32,
}

const MAX_DRAWS: u32 = 256;

impl GBufferPass {
    /// Creates the GBuffer pass for the given surface format and size.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let gbuffer = GBuffer::new(device, width, height);

        let gbuffer_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gbuffer"),
            source: wgpu::ShaderSource::Wgsl(shaders::load(shaders::Shader::GBuffer)),
        });

        let frame_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gbuf_frame"),
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

        let draw_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gbuf_draw"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let tex_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gbuf_textures"),
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let frame_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gbuf_frame_uniforms"),
            size: size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let min_align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let draw_stride = DRAW_UNIFORM_SIZE.div_ceil(min_align) * min_align;
        let draw_buf_size = draw_stride * MAX_DRAWS as u64;

        let draw_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gbuf_draw_uniforms"),
            size: draw_buf_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gbuf_frame"),
            layout: &frame_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_uniform_buf.as_entire_binding(),
            }],
        });

        let draw_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gbuf_draw"),
            layout: &draw_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &draw_uniform_buf,
                    offset: 0,
                    size: Some(wgpu::BufferSize::new(DRAW_UNIFORM_SIZE).unwrap()),
                }),
            }],
        });

        let gbuffer_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("gbuffer"),
                bind_group_layouts: &[
                    Some(&frame_bind_group_layout),
                    Some(&draw_bind_group_layout),
                    Some(&tex_bind_group_layout),
                ],
                immediate_size: 0,
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

        let color_targets = [
            Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            }),
        ];

        let depth_stencil = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };

        let make_pipeline = |label, cull_mode| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&gbuffer_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &gbuffer_module,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(vertex_layout.clone())],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &gbuffer_module,
                    entry_point: Some("fs_main"),
                    targets: &color_targets,
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(depth_stencil.clone()),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let gbuffer_pipeline = make_pipeline("gbuffer", Some(wgpu::Face::Back));
        let gbuffer_pipeline_double_sided = make_pipeline("gbuffer_double_sided", None);

        let tex_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let texture_cache = TextureCache::new(device, queue);
        let hzb = HzbBuilder::new();

        Self {
            gbuffer,
            gbuffer_pipeline,
            gbuffer_pipeline_double_sided,
            frame_bind_group_layout,
            draw_bind_group_layout,
            tex_bind_group_layout,
            frame_uniform_buf,
            draw_uniform_buf,
            frame_bind_group,
            draw_bind_group,
            tex_sampler,
            texture_cache,
            hzb,
            surface_format,
            max_draws: MAX_DRAWS,
        }
    }

    /// Recreates GBuffer textures on resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.gbuffer.width && height == self.gbuffer.height {
            return;
        }
        self.gbuffer = GBuffer::new(device, width, height);
    }

    /// Returns a reference to the GBuffer textures for downstream passes.
    pub fn gbuffer(&self) -> &GBuffer {
        &self.gbuffer
    }

    /// Renders the GBuffer pass and builds HZB.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &FreeCamera,
        world: &World,
        stream_output: &StreamOutput<'_>,
    ) {
        let view_proj = camera.projection_matrix() * camera.view_matrix();

        let frame_uniforms = FrameUniforms {
            view_proj: view_proj.to_cols_array(),
        };
        queue.write_buffer(&self.frame_uniform_buf, 0, bytemuck::bytes_of(&frame_uniforms));

        let min_align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let draw_stride = DRAW_UNIFORM_SIZE.div_ceil(min_align) * min_align;

        struct DrawCall<'a> {
            gpu_mesh: &'a GpuMesh,
            dynamic_offset: u32,
            double_sided: bool,
            albedo_key: Option<TextureKey>,
            normal_key: Option<TextureKey>,
            rm_key: Option<TextureKey>,
        }

        let mut draws: Vec<DrawCall<'_>> = Vec::new();
        let mut draw_index: u32 = 0;

        // Volume meshes
        for (_key, gpu_mesh) in &stream_output.volume_meshes {
            if draw_index >= self.max_draws {
                break;
            }
            let uniforms = DrawUniforms {
                model: Mat4::IDENTITY.to_cols_array(),
                base_color: [0.6, 0.6, 0.6, 1.0],
                roughness_metallic: [0.8, 0.0],
                _pad: [0.0; 2],
            };
            let offset = draw_index as u64 * draw_stride;
            queue.write_buffer(&self.draw_uniform_buf, offset, bytemuck::bytes_of(&uniforms));
            draws.push(DrawCall {
                gpu_mesh,
                dynamic_offset: offset as u32,
                double_sided: false,
                albedo_key: None,
                normal_key: None,
                rm_key: None,
            });
            draw_index += 1;
        }

        // Mesh instances
        for (key, gpu_mesh) in &stream_output.mesh_instances {
            if draw_index >= self.max_draws {
                break;
            }
            let (model, base_color, roughness, metallic, double_sided, albedo_key, normal_key, rm_key) =
                if let Some(inst) = world.mesh_instance(*key) {
                    let model = Mat4::from_scale_rotation_translation(
                        inst.scale,
                        inst.rotation,
                        inst.position,
                    );
                    let mat = world.material(inst.material);
                    let bc = mat.map(|m| m.base_color.to_array()).unwrap_or([1.0; 4]);
                    let rough = mat.map(|m| m.roughness).unwrap_or(0.5);
                    let metal = mat.map(|m| m.metallic).unwrap_or(0.0);
                    let albedo = mat.and_then(|m| m.albedo_map);
                    let normal = mat.and_then(|m| m.normal_map);
                    let rm = mat.and_then(|m| m.roughness_metallic_map);
                    (model, bc, rough, metal, inst.double_sided, albedo, normal, rm)
                } else {
                    (Mat4::IDENTITY, [1.0; 4], 0.5, 0.0, false, None, None, None)
                };

            let uniforms = DrawUniforms {
                model: model.to_cols_array(),
                base_color,
                roughness_metallic: [roughness, metallic],
                _pad: [0.0; 2],
            };
            let offset = draw_index as u64 * draw_stride;
            queue.write_buffer(&self.draw_uniform_buf, offset, bytemuck::bytes_of(&uniforms));
            draws.push(DrawCall {
                gpu_mesh,
                dynamic_offset: offset as u32,
                double_sided,
                albedo_key,
                normal_key,
                rm_key,
            });
            draw_index += 1;
        }

        // Ensure all referenced textures are uploaded
        for draw in &draws {
            for key in [draw.albedo_key, draw.normal_key, draw.rm_key].into_iter().flatten() {
                self.texture_cache.get_or_upload(key, world, device, queue);
            }
        }

        // --- GBuffer render pass ---
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gbuffer"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.gbuffer.albedo_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.gbuffer.normal_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.5,
                                g: 0.5,
                                b: 0.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.gbuffer.material_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.gbuffer.depth_view,
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

            rpass.set_bind_group(0, &self.frame_bind_group, &[]);
            let mut current_double_sided = false;
            rpass.set_pipeline(&self.gbuffer_pipeline);

            for draw in &draws {
                if draw.double_sided != current_double_sided {
                    current_double_sided = draw.double_sided;
                    if current_double_sided {
                        rpass.set_pipeline(&self.gbuffer_pipeline_double_sided);
                    } else {
                        rpass.set_pipeline(&self.gbuffer_pipeline);
                    }
                }

                let albedo_view = self.texture_cache.view_or_fallback(
                    draw.albedo_key,
                    &self.texture_cache.fallback_albedo.view,
                );
                let normal_view = self.texture_cache.view_or_fallback(
                    draw.normal_key,
                    &self.texture_cache.fallback_normal.view,
                );
                let rm_view = self.texture_cache.view_or_fallback(
                    draw.rm_key,
                    &self.texture_cache.fallback_roughness_metallic.view,
                );

                let tex_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("gbuf_tex"),
                    layout: &self.tex_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(albedo_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(normal_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(rm_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.tex_sampler),
                        },
                    ],
                });

                rpass.set_bind_group(1, &self.draw_bind_group, &[draw.dynamic_offset]);
                rpass.set_bind_group(2, &tex_bind_group, &[]);
                rpass.set_vertex_buffer(0, draw.gpu_mesh.vertex_buffer.slice(..));
                rpass.set_index_buffer(
                    draw.gpu_mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                rpass.draw_indexed(0..draw.gpu_mesh.index_count, 0, 0..1);
            }
        }

        // HZB: ensure texture is allocated (build step deferred)
        self.hzb
            .ensure_texture(device, self.gbuffer.width, self.gbuffer.height);
    }
}
