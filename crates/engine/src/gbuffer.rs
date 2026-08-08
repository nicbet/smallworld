//! GBuffer pass: first sub-stages of Execute.
//!
//! Renders visible cached meshes into a GBuffer (albedo, normal,
//! material) with depth. Builds the HZB mip chain from depth for
//! next frame's occlusion culling. Blits albedo to the swapchain
//! as a debug view until the lighting pass lands.

use glam::Mat4;

use crate::camera::FreeCamera;
use crate::mesh::Vertex;
use crate::shaders;
use crate::stream::{GpuMesh, StreamOutput};
use crate::world::World;

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

    // Mip chain build deferred — requires depth→R32Float copy that
    // needs a separate compute pass with texture_depth_2d binding.
    // The CullStage doesn't use HZB yet (passes None). The build
    // step lands when GPU occlusion culling is implemented.

    #[allow(dead_code)]
    fn view(&self) -> Option<wgpu::TextureView> {
        self.texture
            .as_ref()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
    }
}

// ---------------------------------------------------------------------------
// GBufferPass
// ---------------------------------------------------------------------------

/// Renders meshes into the GBuffer, builds HZB, and blits albedo to screen.
pub struct GBufferPass {
    gbuffer: GBuffer,
    gbuffer_pipeline: wgpu::RenderPipeline,
    #[allow(dead_code)]
    frame_bind_group_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)]
    draw_bind_group_layout: wgpu::BindGroupLayout,
    frame_uniform_buf: wgpu::Buffer,
    draw_uniform_buf: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    draw_bind_group: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
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
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let gbuffer = GBuffer::new(device, width, height);

        // --- GBuffer pipeline ---
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

        let gbuffer_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gbuffer"),
            layout: Some(&gbuffer_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &gbuffer_module,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout)],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &gbuffer_module,
                entry_point: Some("fs_main"),
                targets: &[
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
                ],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // --- Blit pipeline (albedo → surface) ---
        let blit_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug_blit"),
            source: wgpu::ShaderSource::Wgsl(shaders::load(shaders::Shader::Blit)),
        });

        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("debug_blit"),
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
            label: Some("debug_blit"),
            bind_group_layouts: &[Some(&blit_bind_group_layout)],
            immediate_size: 0,
        });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug_blit"),
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
            label: Some("debug_blit"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let hzb = HzbBuilder::new();

        Self {
            gbuffer,
            gbuffer_pipeline,
            frame_bind_group_layout,
            draw_bind_group_layout,
            frame_uniform_buf,
            draw_uniform_buf,
            frame_bind_group,
            draw_bind_group,
            blit_pipeline,
            blit_bind_group_layout,
            blit_sampler,
            hzb,
            surface_format,
            max_draws: MAX_DRAWS,
        }
    }

    /// Recreates GBuffer textures and blit bind group on resize.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.gbuffer.width && height == self.gbuffer.height {
            return;
        }
        self.gbuffer = GBuffer::new(device, width, height);
    }

    /// Renders the GBuffer pass, builds HZB, and blits albedo to the surface.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        camera: &FreeCamera,
        world: &World,
        stream_output: &StreamOutput<'_>,
    ) {
        let view_proj = camera.projection_matrix() * camera.view_matrix();

        // Write frame uniforms
        let frame_uniforms = FrameUniforms {
            view_proj: view_proj.to_cols_array(),
        };
        queue.write_buffer(&self.frame_uniform_buf, 0, bytemuck::bytes_of(&frame_uniforms));

        // Collect draws and write per-draw uniforms
        let min_align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let draw_stride = DRAW_UNIFORM_SIZE.div_ceil(min_align) * min_align;

        struct DrawCall<'a> {
            gpu_mesh: &'a GpuMesh,
            dynamic_offset: u32,
        }

        let mut draws: Vec<DrawCall<'_>> = Vec::new();
        let mut draw_index: u32 = 0;

        // Volume meshes (identity model matrix, white material)
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
            });
            draw_index += 1;
        }

        // Mesh instance meshes
        for (key, gpu_mesh) in &stream_output.mesh_instances {
            if draw_index >= self.max_draws {
                break;
            }
            let (model, base_color, roughness, metallic) =
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
                    (model, bc, rough, metal)
                } else {
                    (Mat4::IDENTITY, [1.0; 4], 0.5, 0.0)
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
            });
            draw_index += 1;
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

            rpass.set_pipeline(&self.gbuffer_pipeline);
            rpass.set_bind_group(0, &self.frame_bind_group, &[]);

            for draw in &draws {
                rpass.set_bind_group(1, &self.draw_bind_group, &[draw.dynamic_offset]);
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

        // --- Debug blit: albedo → surface ---
        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug_blit"),
            layout: &self.blit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.gbuffer.albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blit_sampler),
                },
            ],
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("debug_blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.02,
                            b: 0.08,
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
            rpass.set_pipeline(&self.blit_pipeline);
            rpass.set_bind_group(0, &blit_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}
