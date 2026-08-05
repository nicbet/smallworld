//! Compute-shader raymarcher for a dense 256³ voxel volume.

use crate::camera::FreeCamera;
use crate::gpu::GpuContext;
use crate::shaders::{self, Shader};

const VOLUME_EDGE: usize = 256;
const VOLUME_LEN: usize = VOLUME_EDGE * VOLUME_EDGE * VOLUME_EDGE;
const WORKGROUP_SIZE: u32 = 8;

/// GPU-resident raymarcher: compute pass writes to a storage texture, blit pass
/// copies the result to the surface.
pub struct Raymarcher {
    compute_pipeline: wgpu::ComputePipeline,
    compute_bind_group_layout: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    compute_bind_group: wgpu::BindGroup,
    blit_bind_group: wgpu::BindGroup,
    camera_buf: wgpu::Buffer,
    #[allow(dead_code)]
    voxel_buf: wgpu::Buffer,
    #[allow(dead_code)]
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniforms {
    inv_view_proj: [f32; 16],
    camera_pos: [f32; 4],
    resolution: [f32; 2],
    _pad: [f32; 2],
}

impl Raymarcher {
    /// Creates pipelines, allocates the voxel buffer with a procedural test scene,
    /// and creates the output texture at the given resolution.
    pub fn new(
        gpu: &GpuContext,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let compute_source = shaders::compose(&[Shader::Common, Shader::Raymarch]);
        let compute_module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("raymarch"),
                source: wgpu::ShaderSource::Wgsl(compute_source.into()),
            });

        let blit_source = shaders::load(Shader::Blit);
        let blit_module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("blit"),
                source: wgpu::ShaderSource::Wgsl(blit_source),
            });

        // --- Compute pipeline ---
        let compute_bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("raymarch"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::StorageTexture {
                                access: wgpu::StorageTextureAccess::WriteOnly,
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                view_dimension: wgpu::TextureViewDimension::D2,
                            },
                            count: None,
                        },
                    ],
                });

        let compute_pipeline_layout =
            gpu.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("raymarch"),
                    bind_group_layouts: &[Some(&compute_bind_group_layout)],
                    immediate_size: 0,
                });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("raymarch"),
                    layout: Some(&compute_pipeline_layout),
                    module: &compute_module,
                    entry_point: Some("cs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });

        // --- Blit pipeline ---
        let blit_bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("blit"),
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

        let blit_pipeline_layout =
            gpu.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("blit"),
                    bind_group_layouts: &[Some(&blit_bind_group_layout)],
                    immediate_size: 0,
                });

        let blit_pipeline =
            gpu.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("blit"),
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

        // --- Buffers ---
        let camera_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera_uniforms"),
            size: size_of::<CameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let voxel_data = generate_test_volume();
        let voxel_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel_data"),
            size: (VOLUME_LEN * size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&voxel_buf, 0, bytemuck::cast_slice(&voxel_data));

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // --- Output texture + bind groups ---
        let (output_texture, output_view) = create_output_texture(&gpu.device, width, height);
        let compute_bind_group = create_compute_bind_group(
            &gpu.device,
            &compute_bind_group_layout,
            &camera_buf,
            &voxel_buf,
            &output_view,
        );
        let blit_bind_group = create_blit_bind_group(
            &gpu.device,
            &blit_bind_group_layout,
            &output_view,
            &sampler,
        );

        Self {
            compute_pipeline,
            compute_bind_group_layout,
            blit_pipeline,
            blit_bind_group_layout,
            compute_bind_group,
            blit_bind_group,
            camera_buf,
            voxel_buf,
            output_texture,
            output_view,
            sampler,
            width,
            height,
        }
    }

    /// Recreates the output texture and bind groups at a new resolution.
    pub fn resize(&mut self, gpu: &GpuContext, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;

        let (tex, view) = create_output_texture(&gpu.device, width, height);
        self.output_texture = tex;
        self.output_view = view;

        self.compute_bind_group = create_compute_bind_group(
            &gpu.device,
            &self.compute_bind_group_layout,
            &self.camera_buf,
            &self.voxel_buf,
            &self.output_view,
        );
        self.blit_bind_group = create_blit_bind_group(
            &gpu.device,
            &self.blit_bind_group_layout,
            &self.output_view,
            &self.sampler,
        );
    }

    /// Dispatches the compute raymarch pass and blits the result to `surface_view`.
    pub fn render(
        &self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        camera: &FreeCamera,
    ) {
        let vp = camera.projection_matrix() * camera.view_matrix();
        let inv_vp = vp.inverse();
        let uniforms = CameraUniforms {
            inv_view_proj: inv_vp.to_cols_array(),
            camera_pos: [
                camera.position.x,
                camera.position.y,
                camera.position.z,
                1.0,
            ],
            resolution: [self.width as f32, self.height as f32],
            _pad: [0.0; 2],
        };
        gpu.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniforms));

        // Compute pass
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("raymarch"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &self.compute_bind_group, &[]);
            cpass.dispatch_workgroups(
                self.width.div_ceil(WORKGROUP_SIZE),
                self.height.div_ceil(WORKGROUP_SIZE),
                1,
            );
        }

        // Blit pass
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blit"),
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
            rpass.set_bind_group(0, &self.blit_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

fn create_output_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("raymarch_output"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_compute_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_buf: &wgpu::Buffer,
    voxel_buf: &wgpu::Buffer,
    output_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raymarch"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: voxel_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output_view),
            },
        ],
    })
}

fn create_blit_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    output_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(output_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn generate_test_volume() -> Vec<u32> {
    let mut data = vec![0u32; VOLUME_LEN];
    let edge = VOLUME_EDGE as i32;
    let center = edge / 2;
    let sphere_radius_sq = 40 * 40;
    let ground_height = 80;

    for z in 0..edge {
        for y in 0..edge {
            for x in 0..edge {
                let idx = (x + edge * (y + edge * z)) as usize;

                let dx = x - center;
                let dy = y - center;
                let dz = z - center;
                if dx * dx + dy * dy + dz * dz <= sphere_radius_sq {
                    data[idx] = 2;
                } else if y < ground_height {
                    data[idx] = 1;
                }
            }
        }
    }

    data
}
