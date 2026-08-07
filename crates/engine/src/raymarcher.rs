//! Compute-shader raymarcher over a sparse brick grid.

use crate::brick_pool::BrickPool;
use crate::camera::FreeCamera;
use crate::gpu::GpuContext;
use crate::shaders::{self, Shader};
use crate::svo::Svo;
use crate::world::WorldGpuData;

const WORKGROUP_SIZE: u32 = 8;

/// GPU-resident raymarcher, split into three compute passes so shadow rays
/// run warp-dense at half resolution:
///
/// 1. `cs_primary` (full res): trace, write G-buffer (position+ndotl, albedo,
///    normal).
/// 2. `cs_shadow` (half res): one shadow ray per 2×2 quad from the G-buffer.
///    Skipped entirely when shadows are off.
/// 3. `cs_shade` (full res): combine G-buffer and shadow into the output.
///
/// A blit pass then copies the output to the surface.
pub struct Raymarcher {
    primary_pipeline: wgpu::ComputePipeline,
    shadow_pipeline: wgpu::ComputePipeline,
    shade_pipeline: wgpu::ComputePipeline,
    primary_bgl: wgpu::BindGroupLayout,
    shadow_bgl: wgpu::BindGroupLayout,
    shade_bgl: wgpu::BindGroupLayout,
    primary_bg: wgpu::BindGroup,
    shadow_bg: wgpu::BindGroup,
    shade_bg: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group_layout: wgpu::BindGroupLayout,
    blit_bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    dummy_buf: wgpu::Buffer,
    targets: RenderTargets,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
    /// World-space Y above which the terrain SVO holds no solid content.
    /// Shadow rays prune traversal past their exit from this slab. Defaults
    /// to the world-cube top (no pruning) until the world builder calls
    /// [`set_terrain_top_y`](Self::set_terrain_top_y).
    terrain_top_y: f32,
}

/// Output, G-buffer and shadow textures — recreated together on resize.
/// Textures are kept alive alongside their views.
struct RenderTargets {
    #[allow(dead_code)]
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    #[allow(dead_code)]
    gbuf_pos_tex: wgpu::Texture,
    gbuf_pos_view: wgpu::TextureView,
    #[allow(dead_code)]
    gbuf_albedo_tex: wgpu::Texture,
    gbuf_albedo_view: wgpu::TextureView,
    #[allow(dead_code)]
    gbuf_norm_tex: wgpu::Texture,
    gbuf_norm_view: wgpu::TextureView,
    #[allow(dead_code)]
    shadow_tex: wgpu::Texture,
    shadow_view: wgpu::TextureView,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    inv_view_proj: [f32; 16],
    camera_pos: [f32; 4],
    resolution: [f32; 2],
    _pad0: [f32; 2],
    world_min: [f32; 3],
    world_size: f32,
    terrain_top_y: f32,
    _pad1: [f32; 2],
    flags: u32,
    instance_count: u32,
    focal_length: f32,
    sse_threshold: f32,
    svo_root: u32,
}

impl Raymarcher {
    /// Creates pipelines and bind groups referencing the brick pool and index.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpu: &GpuContext,
        width: u32,
        height: u32,
        surface_format: wgpu::TextureFormat,
        pool: &BrickPool,
        svo: &Svo,
        world_data: &WorldGpuData,
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

        // --- Compute bind group layouts, one per pass ---
        let primary_bgl = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("raymarch_primary"),
                entries: &[
                    bgl_entry(0, wgpu::ShaderStages::COMPUTE, uniform_binding()),
                    bgl_entry(1, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(2, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(3, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(5, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(6, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(7, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(8, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(
                        9,
                        wgpu::ShaderStages::COMPUTE,
                        storage_tex_binding(wgpu::TextureFormat::Rgba32Float),
                    ),
                    bgl_entry(
                        10,
                        wgpu::ShaderStages::COMPUTE,
                        storage_tex_binding(wgpu::TextureFormat::Rgba8Unorm),
                    ),
                    bgl_entry(
                        11,
                        wgpu::ShaderStages::COMPUTE,
                        storage_tex_binding(wgpu::TextureFormat::Rgba8Snorm),
                    ),
                ],
            });

        let shadow_bgl = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("raymarch_shadow"),
                entries: &[
                    bgl_entry(0, wgpu::ShaderStages::COMPUTE, uniform_binding()),
                    bgl_entry(1, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(2, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(5, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(6, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(7, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(8, wgpu::ShaderStages::COMPUTE, storage_ro_binding()),
                    bgl_entry(12, wgpu::ShaderStages::COMPUTE, texture_ro_binding()),
                    bgl_entry(14, wgpu::ShaderStages::COMPUTE, texture_ro_binding()),
                    bgl_entry(
                        15,
                        wgpu::ShaderStages::COMPUTE,
                        storage_tex_binding(wgpu::TextureFormat::R32Float),
                    ),
                ],
            });

        let shade_bgl = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("raymarch_shade"),
                entries: &[
                    bgl_entry(0, wgpu::ShaderStages::COMPUTE, uniform_binding()),
                    bgl_entry(
                        4,
                        wgpu::ShaderStages::COMPUTE,
                        storage_tex_binding(wgpu::TextureFormat::Rgba8Unorm),
                    ),
                    bgl_entry(12, wgpu::ShaderStages::COMPUTE, texture_ro_binding()),
                    bgl_entry(13, wgpu::ShaderStages::COMPUTE, texture_ro_binding()),
                    bgl_entry(16, wgpu::ShaderStages::COMPUTE, texture_ro_binding()),
                ],
            });

        let make_pipeline = |label: &str, bgl: &wgpu::BindGroupLayout, entry: &str| {
            let layout = gpu
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: &[Some(bgl)],
                    immediate_size: 0,
                });
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    module: &compute_module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                })
        };
        let primary_pipeline = make_pipeline("raymarch_primary", &primary_bgl, "cs_primary");
        let shadow_pipeline = make_pipeline("raymarch_shadow", &shadow_bgl, "cs_shadow");
        let shade_pipeline = make_pipeline("raymarch_shade", &shade_bgl, "cs_shade");

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

        let blit_pipeline = gpu
            .device
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
            size: 256,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // --- Render targets + bind groups ---
        let targets = create_render_targets(&gpu.device, width, height);
        let instance_buf = world_data.instance_buffer().unwrap_or(&dummy_buf);
        let grid_buf = world_data.grid_buffer().unwrap_or(&dummy_buf);
        let bvh_buf = world_data.bvh_buffer().unwrap_or(&dummy_buf);
        let (primary_bg, shadow_bg, shade_bg) = create_pass_bind_groups(
            &gpu.device,
            [&primary_bgl, &shadow_bgl, &shade_bgl],
            &uniform_buf,
            svo.buffer(),
            pool.voxel_buffer(),
            pool.palette_buffer(),
            instance_buf,
            grid_buf,
            bvh_buf,
            pool.mask_buffer(),
            &targets,
        );
        let blit_bind_group = create_blit_bind_group(
            &gpu.device,
            &blit_bind_group_layout,
            &targets.output_view,
            &sampler,
        );

        Self {
            primary_pipeline,
            shadow_pipeline,
            shade_pipeline,
            primary_bgl,
            shadow_bgl,
            shade_bgl,
            primary_bg,
            shadow_bg,
            shade_bg,
            blit_pipeline,
            blit_bind_group_layout,
            blit_bind_group,
            uniform_buf,
            dummy_buf,
            targets,
            sampler,
            width,
            height,
            terrain_top_y: svo.world_min().y + svo.world_size(),
        }
    }

    /// Declares the world-space Y above which the terrain SVO is guaranteed
    /// empty (top of the terrain brick grid). Shadow rays stop traversing
    /// once they exit this slab — exact, because terrain content cannot
    /// exist above it. Instanced objects are unaffected (separate BVH path).
    pub fn set_terrain_top_y(&mut self, y: f32) {
        self.terrain_top_y = y;
    }

    /// Recreates the output texture and bind groups at a new resolution.
    #[allow(clippy::too_many_arguments)]
    pub fn resize(
        &mut self,
        gpu: &GpuContext,
        width: u32,
        height: u32,
        pool: &BrickPool,
        svo: &Svo,
        world_data: &WorldGpuData,
    ) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;

        self.targets = create_render_targets(&gpu.device, width, height);

        let instance_buf = world_data.instance_buffer().unwrap_or(&self.dummy_buf);
        let grid_buf = world_data.grid_buffer().unwrap_or(&self.dummy_buf);
        let bvh_buf = world_data.bvh_buffer().unwrap_or(&self.dummy_buf);
        let (primary_bg, shadow_bg, shade_bg) = create_pass_bind_groups(
            &gpu.device,
            [&self.primary_bgl, &self.shadow_bgl, &self.shade_bgl],
            &self.uniform_buf,
            svo.buffer(),
            pool.voxel_buffer(),
            pool.palette_buffer(),
            instance_buf,
            grid_buf,
            bvh_buf,
            pool.mask_buffer(),
            &self.targets,
        );
        self.primary_bg = primary_bg;
        self.shadow_bg = shadow_bg;
        self.shade_bg = shade_bg;
        self.blit_bind_group = create_blit_bind_group(
            &gpu.device,
            &self.blit_bind_group_layout,
            &self.targets.output_view,
            &self.sampler,
        );
    }

    /// Dispatches the compute raymarch pass and blits the result to `surface_view`.
    /// Flag: enable sun shadow rays.
    pub const FLAG_SHADOWS: u32 = 1;
    /// Flag: enable smooth normals from occupancy gradient.
    pub const FLAG_SMOOTH_NORMALS: u32 = 2;

    /// Dispatches the compute raymarch pass and blits the result to `surface_view`.
    ///
    /// Both passes go into one encoder. For per-pass GPU timing on Metal use
    /// [`compute_pass`](Self::compute_pass) / [`blit_pass`](Self::blit_pass)
    /// with separate encoders — Metal resolves pass-boundary timestamps at
    /// command-buffer granularity, so passes sharing a buffer all report the
    /// buffer's total time.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        camera: &FreeCamera,
        svo: &Svo,
        world_data: &WorldGpuData,
        flags: u32,
        sse_threshold: f32,
        compute_timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
        blit_timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        self.compute_pass(
            gpu,
            encoder,
            camera,
            svo,
            world_data,
            flags,
            sse_threshold,
            compute_timestamps,
        );
        self.blit_pass(encoder, surface_view, blit_timestamps);
    }

    /// Writes uniforms and dispatches the raymarch compute pass.
    #[allow(clippy::too_many_arguments)]
    pub fn compute_pass(
        &self,
        gpu: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        camera: &FreeCamera,
        svo: &Svo,
        world_data: &WorldGpuData,
        flags: u32,
        sse_threshold: f32,
        compute_timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        let vp = camera.projection_matrix() * camera.view_matrix();
        let inv_vp = vp.inverse();
        let wmin = svo.world_min();
        let uniforms = Uniforms {
            inv_view_proj: inv_vp.to_cols_array(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            resolution: [self.width as f32, self.height as f32],
            _pad0: [0.0; 2],
            world_min: [wmin.x, wmin.y, wmin.z],
            world_size: svo.world_size(),
            terrain_top_y: self.terrain_top_y,
            _pad1: [0.0; 2],
            flags,
            instance_count: world_data.instance_count(),
            focal_length: self.height as f32 / (2.0 * (camera.fov_y * 0.5).tan()),
            sse_threshold,
            svo_root: svo.root(),
        };
        gpu.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Three passes must be separate: a storage-texture write and its
        // texture read cannot share one usage scope. The caller's timestamp
        // pair is split so it still brackets the whole compute workload.
        let (ts_begin, ts_end) = match &compute_timestamps {
            Some(ts) => (
                Some(wgpu::ComputePassTimestampWrites {
                    query_set: ts.query_set,
                    beginning_of_pass_write_index: ts.beginning_of_pass_write_index,
                    end_of_pass_write_index: None,
                }),
                Some(wgpu::ComputePassTimestampWrites {
                    query_set: ts.query_set,
                    beginning_of_pass_write_index: None,
                    end_of_pass_write_index: ts.end_of_pass_write_index,
                }),
            ),
            None => (None, None),
        };

        let full_x = self.width.div_ceil(WORKGROUP_SIZE);
        let full_y = self.height.div_ceil(WORKGROUP_SIZE);

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("raymarch_primary"),
                timestamp_writes: ts_begin,
            });
            cpass.set_pipeline(&self.primary_pipeline);
            cpass.set_bind_group(0, &self.primary_bg, &[]);
            cpass.dispatch_workgroups(full_x, full_y, 1);
        }

        if flags & Self::FLAG_SHADOWS != 0 {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("raymarch_shadow"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.shadow_pipeline);
            cpass.set_bind_group(0, &self.shadow_bg, &[]);
            cpass.dispatch_workgroups(
                self.width.div_ceil(2).div_ceil(WORKGROUP_SIZE),
                self.height.div_ceil(2).div_ceil(WORKGROUP_SIZE),
                1,
            );
        }

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("raymarch_shade"),
                timestamp_writes: ts_end,
            });
            cpass.set_pipeline(&self.shade_pipeline);
            cpass.set_bind_group(0, &self.shade_bg, &[]);
            cpass.dispatch_workgroups(full_x, full_y, 1);
        }
    }

    /// Blits the compute output to `surface_view`.
    pub fn blit_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        blit_timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
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

fn storage_tex_binding(format: wgpu::TextureFormat) -> wgpu::BindingType {
    wgpu::BindingType::StorageTexture {
        access: wgpu::StorageTextureAccess::WriteOnly,
        format,
        view_dimension: wgpu::TextureViewDimension::D2,
    }
}

fn texture_ro_binding() -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: false },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}

fn create_target(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_render_targets(device: &wgpu::Device, width: u32, height: u32) -> RenderTargets {
    let (output_texture, output_view) = create_target(
        device,
        "raymarch_output",
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
    );
    let (gbuf_pos_tex, gbuf_pos_view) = create_target(
        device,
        "gbuf_pos",
        width,
        height,
        wgpu::TextureFormat::Rgba32Float,
    );
    let (gbuf_albedo_tex, gbuf_albedo_view) = create_target(
        device,
        "gbuf_albedo",
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
    );
    let (gbuf_norm_tex, gbuf_norm_view) = create_target(
        device,
        "gbuf_norm",
        width,
        height,
        wgpu::TextureFormat::Rgba8Snorm,
    );
    let (shadow_tex, shadow_view) = create_target(
        device,
        "shadow_half",
        width.div_ceil(2).max(1),
        height.div_ceil(2).max(1),
        wgpu::TextureFormat::R32Float,
    );
    RenderTargets {
        output_texture,
        output_view,
        gbuf_pos_tex,
        gbuf_pos_view,
        gbuf_albedo_tex,
        gbuf_albedo_view,
        gbuf_norm_tex,
        gbuf_norm_view,
        shadow_tex,
        shadow_view,
    }
}

#[allow(clippy::too_many_arguments)]
fn create_pass_bind_groups(
    device: &wgpu::Device,
    layouts: [&wgpu::BindGroupLayout; 3],
    uniform_buf: &wgpu::Buffer,
    index_buf: &wgpu::Buffer,
    voxel_buf: &wgpu::Buffer,
    palette_buf: &wgpu::Buffer,
    instance_buf: &wgpu::Buffer,
    object_grid_buf: &wgpu::Buffer,
    bvh_buf: &wgpu::Buffer,
    mask_buf: &wgpu::Buffer,
    targets: &RenderTargets,
) -> (wgpu::BindGroup, wgpu::BindGroup, wgpu::BindGroup) {
    fn buf(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: b.as_entire_binding(),
        }
    }
    fn tex(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(v),
        }
    }

    let primary = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raymarch_primary"),
        layout: layouts[0],
        entries: &[
            buf(0, uniform_buf),
            buf(1, index_buf),
            buf(2, voxel_buf),
            buf(3, palette_buf),
            buf(5, instance_buf),
            buf(6, object_grid_buf),
            buf(7, bvh_buf),
            buf(8, mask_buf),
            tex(9, &targets.gbuf_pos_view),
            tex(10, &targets.gbuf_albedo_view),
            tex(11, &targets.gbuf_norm_view),
        ],
    });

    let shadow = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raymarch_shadow"),
        layout: layouts[1],
        entries: &[
            buf(0, uniform_buf),
            buf(1, index_buf),
            buf(2, voxel_buf),
            buf(5, instance_buf),
            buf(6, object_grid_buf),
            buf(7, bvh_buf),
            buf(8, mask_buf),
            tex(12, &targets.gbuf_pos_view),
            tex(14, &targets.gbuf_norm_view),
            tex(15, &targets.shadow_view),
        ],
    });

    let shade = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("raymarch_shade"),
        layout: layouts[2],
        entries: &[
            buf(0, uniform_buf),
            tex(4, &targets.output_view),
            tex(12, &targets.gbuf_pos_view),
            tex(13, &targets.gbuf_albedo_view),
            tex(16, &targets.shadow_view),
        ],
    });

    (primary, shadow, shade)
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
