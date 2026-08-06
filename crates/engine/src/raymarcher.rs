//! Compute-shader raymarcher over a sparse brick grid.

use crate::brick_index::BrickIndex;
use crate::brick_pool::BrickPool;
use crate::camera::FreeCamera;
use crate::gpu::GpuContext;
use crate::scene::Scene;
use crate::shaders::{self, Shader};

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
    uniform_buf: wgpu::Buffer,
    dummy_buf: wgpu::Buffer,
    #[allow(dead_code)]
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    inv_view_proj: [f32; 16],
    camera_pos: [f32; 4],
    resolution: [f32; 2],
    _pad0: [f32; 2],
    world_min: [f32; 3],
    brick_size: f32,
    grid_dims: [u32; 3],
    flags: u32,
    instance_count: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
}

impl Raymarcher {
    /// Creates pipelines and bind groups referencing the brick pool and index.
    pub fn new(
        gpu: &GpuContext,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
        pool: &BrickPool,
        index: &BrickIndex,
        scene: &Scene,
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

        // --- Compute bind group layout (7 bindings) ---
        let compute_bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("raymarch"),
                    entries: &[
                        bgl_entry(0, wgpu::ShaderStages::COMPUTE, uniform_binding()),
                        bgl_entry(1, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                        bgl_entry(2, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                        bgl_entry(3, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                        bgl_entry(
                            4,
                            wgpu::ShaderStages::COMPUTE,
                            wgpu::BindingType::StorageTexture {
                                access: wgpu::StorageTextureAccess::WriteOnly,
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                view_dimension: wgpu::TextureViewDimension::D2,
                            },
                        ),
                        bgl_entry(5, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                        bgl_entry(6, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                        bgl_entry(7, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
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

        // --- Blit pipeline (unchanged) ---
        let blit_bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("blit"),
                    entries: &[
                        bgl_entry(
                            0,
                            wgpu::ShaderStages::FRAGMENT,
                            wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                        ),
                        bgl_entry(
                            1,
                            wgpu::ShaderStages::FRAGMENT,
                            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        ),
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
        let uniform_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("raymarch_uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let dummy_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dummy"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // --- Output texture + bind groups ---
        let (output_texture, output_view) = create_output_texture(&gpu.device, width, height);
        let instance_buf = scene.instance_buffer().unwrap_or(&dummy_buf);
        let grid_buf = scene.grid_buffer().unwrap_or(&dummy_buf);
        let bvh_buf = scene.bvh_buffer().unwrap_or(&dummy_buf);
        let compute_bind_group = create_compute_bind_group(
            &gpu.device,
            &compute_bind_group_layout,
            &uniform_buf,
            index.buffer(),
            pool.voxel_buffer(),
            pool.palette_buffer(),
            &output_view,
            instance_buf,
            grid_buf,
            bvh_buf,
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
            uniform_buf,
            dummy_buf,
            output_texture,
            output_view,
            sampler,
            width,
            height,
        }
    }

    /// Recreates the output texture and bind groups at a new resolution.
    pub fn resize(
        &mut self,
        gpu: &GpuContext,
        width: u32,
        height: u32,
        pool: &BrickPool,
        index: &BrickIndex,
        scene: &Scene,
    ) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;

        let (tex, view) = create_output_texture(&gpu.device, width, height);
        self.output_texture = tex;
        self.output_view = view;

        let instance_buf = scene.instance_buffer().unwrap_or(&self.dummy_buf);
        let grid_buf = scene.grid_buffer().unwrap_or(&self.dummy_buf);
        let bvh_buf = scene.bvh_buffer().unwrap_or(&self.dummy_buf);
        self.compute_bind_group = create_compute_bind_group(
            &gpu.device,
            &self.compute_bind_group_layout,
            &self.uniform_buf,
            index.buffer(),
            pool.voxel_buffer(),
            pool.palette_buffer(),
            &self.output_view,
            instance_buf,
            grid_buf,
            bvh_buf,
        );
        self.blit_bind_group = create_blit_bind_group(
            &gpu.device,
            &self.blit_bind_group_layout,
            &self.output_view,
            &self.sampler,
        );
    }

    /// Dispatches the compute raymarch pass and blits the result to `surface_view`.
    /// Flag: enable sun shadow rays.
    pub const FLAG_SHADOWS: u32 = 1;
    /// Flag: enable smooth normals from occupancy gradient.
    pub const FLAG_SMOOTH_NORMALS: u32 = 2;

    /// Dispatches the compute raymarch pass and blits the result to `surface_view`.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        camera: &FreeCamera,
        index: &BrickIndex,
        scene: &Scene,
        flags: u32,
        compute_timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
        blit_timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let vp = camera.projection_matrix() * camera.view_matrix();
        let inv_vp = vp.inverse();
        let wmin = index.world_min();
        let dims = index.dims();
        let uniforms = Uniforms {
            inv_view_proj: inv_vp.to_cols_array(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            resolution: [self.width as f32, self.height as f32],
            _pad0: [0.0; 2],
            world_min: [wmin.x, wmin.y, wmin.z],
            brick_size: index.brick_size(),
            grid_dims: dims,
            flags,
            instance_count: scene.instance_count(),
            _pad1: 0,
            _pad2: 0,
            _pad3: 0,
        };
        gpu.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Compute pass
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("raymarch"),
                timestamp_writes: compute_timestamps,
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
                timestamp_writes: blit_timestamps,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(&self.blit_pipeline);
            rpass.set_bind_group(0, &self.blit_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

// --- helpers ---

fn bgl_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn uniform_binding() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

fn storage_ro_binding() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only: true },
        has_dynamic_offset: false,
        min_binding_size: None,
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

#[allow(clippy::too_many_arguments)]
fn create_compute_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    index_buf: &wgpu::Buffer,
    voxel_buf: &wgpu::Buffer,
    palette_buf: &wgpu::Buffer,
    output_view: &wgpu::TextureView,
    instance_buf: &wgpu::Buffer,
    object_grid_buf: &wgpu::Buffer,
    bvh_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raymarch"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: index_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: voxel_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: palette_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(output_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: instance_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: object_grid_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: bvh_buf.as_entire_binding(),
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
