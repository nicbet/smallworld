//! GPU-accelerated terrain generation via compute shader.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smallworld_engine::brick_data::BrickData;
use smallworld_engine::brick_pool::{BRICK_EDGE, VOXEL_SCALE};
use smallworld_engine::wgpu;

use crate::worldgen::PALETTE;

const BATCH_SIZE: u32 = 256;
const WORDS_PER_BRICK: u32 = 1024;
const SHADER_SOURCE: &str = include_str!("../shaders/worldgen.wgsl");

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct GenParams {
    seed: u32,
    terrain_base: f32,
    terrain_amp: f32,
    cave_threshold: f32,
    water_level: f32,
    brick_size: f32,
    _pad0: u32,
    _pad1: u32,
    world_min: [f32; 3],
    _pad2: u32,
}

/// GPU-accelerated terrain generator.
///
/// Dispatches a compute shader that evaluates noise and writes packed voxel
/// data. Results are read back via a staging buffer and inserted into a
/// concurrent cache that [`GpuCachedSource`](super::gpu_cached_source::GpuCachedSource)
/// polls from worker threads.
pub struct GpuWorldGenerator {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    param_buf: wgpu::Buffer,
    request_buf: wgpu::Buffer,
    output_buf: wgpu::Buffer,
    staging_buf: wgpu::Buffer,
    cache: Arc<Mutex<HashMap<[u32; 3], Option<BrickData>>>>,
    world_min: glam::Vec3,
    seed: u32,
}

impl GpuWorldGenerator {
    /// Creates the compute pipeline and allocates buffers.
    pub fn new(device: &wgpu::Device, seed: u32, world_min: glam::Vec3) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("worldgen"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("worldgen"),
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("worldgen"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("worldgen"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_generate"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let param_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("worldgen_params"),
            size: size_of::<GenParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let request_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("worldgen_requests"),
            size: u64::from(BATCH_SIZE) * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let output_size = u64::from(BATCH_SIZE) * u64::from(WORDS_PER_BRICK) * 4;
        let output_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("worldgen_output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let staging_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("worldgen_staging"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            param_buf,
            request_buf,
            output_buf,
            staging_buf,
            cache: Arc::new(Mutex::new(HashMap::new())),
            world_min,
            seed,
        }
    }

    /// Shared handle to the results cache — clone this for `GpuCachedSource`.
    pub fn cache(&self) -> Arc<Mutex<HashMap<[u32; 3], Option<BrickData>>>> {
        Arc::clone(&self.cache)
    }

    /// Generates all grid cells synchronously (blocks until done).
    /// Generates full 16³ voxel data for cells within `radius` of `center`.
    /// Results go into the GPU cache for the pager's workers to pull from.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_near(
        &mut self,
        center: glam::Vec3,
        radius: f32,
        dims: [u32; 3],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let start = std::time::Instant::now();
        let r2 = radius * radius;
        let brick_size = BRICK_EDGE as f32 * VOXEL_SCALE;

        let mut positions: Vec<[u32; 3]> = Vec::new();
        for gz in 0..dims[2] {
            for gy in 0..dims[1] {
                for gx in 0..dims[0] {
                    let cell_center = self.world_min
                        + glam::Vec3::new(
                            (gx as f32 + 0.5) * brick_size,
                            (gy as f32 + 0.5) * brick_size,
                            (gz as f32 + 0.5) * brick_size,
                        );
                    if cell_center.distance_squared(center) <= r2 {
                        positions.push([gx, gy, gz]);
                    }
                }
            }
        }

        let total = positions.len();
        for chunk in positions.chunks(BATCH_SIZE as usize) {
            self.dispatch_and_read(chunk, device, queue);
        }

        let elapsed = start.elapsed();
        log::info!(
            "GPU near gen({radius:.0}m): {total} cells in {:.0} ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }

    fn dispatch_and_read(
        &self,
        positions: &[[u32; 3]],
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let count = positions.len() as u32;

        let params = GenParams {
            seed: self.seed,
            terrain_base: 2.0,
            terrain_amp: 8.0,
            cave_threshold: 0.48,
            water_level: -1.0,
            brick_size: BRICK_EDGE as f32 * VOXEL_SCALE,
            _pad0: 0,
            _pad1: 0,
            world_min: self.world_min.into(),
            _pad2: 0,
        };
        queue.write_buffer(&self.param_buf, 0, bytemuck::bytes_of(&params));

        let mut request_data = vec![[0u32; 4]; BATCH_SIZE as usize];
        for (i, pos) in positions.iter().enumerate() {
            request_data[i] = [pos[0], pos[1], pos[2], 0];
        }
        queue.write_buffer(&self.request_buf, 0, bytemuck::cast_slice(&request_data));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("worldgen"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.param_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.request_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.output_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("worldgen"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("worldgen"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            cpass.dispatch_workgroups(count, 1, 1);
        }

        let copy_size = u64::from(count) * u64::from(WORDS_PER_BRICK) * 4;
        encoder.copy_buffer_to_buffer(&self.output_buf, 0, &self.staging_buf, 0, copy_size);
        queue.submit(std::iter::once(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.staging_buf
            .slice(..copy_size)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();

        {
            let view = self
                .staging_buf
                .slice(..copy_size)
                .get_mapped_range()
                .expect("staging mapped");
            let words: &[u32] = bytemuck::cast_slice(&view);

            let mut cache = self.cache.lock().unwrap();
            for (i, pos) in positions.iter().enumerate() {
                let brick_words =
                    &words[i * WORDS_PER_BRICK as usize..(i + 1) * WORDS_PER_BRICK as usize];

                let is_air = brick_words.iter().all(|&w| w == 0);
                if is_air {
                    cache.insert(*pos, None);
                } else {
                    let mut voxels = [0u8; 4096];
                    for (wi, &word) in brick_words.iter().enumerate() {
                        let base = wi * 4;
                        voxels[base] = (word & 0xFF) as u8;
                        voxels[base + 1] = ((word >> 8) & 0xFF) as u8;
                        voxels[base + 2] = ((word >> 16) & 0xFF) as u8;
                        voxels[base + 3] = ((word >> 24) & 0xFF) as u8;
                    }
                    cache.insert(
                        *pos,
                        Some(BrickData {
                            voxels,
                            palette: PALETTE.to_vec(),
                        }),
                    );
                }
            }
        }
        self.staging_buf.unmap();
    }
}
