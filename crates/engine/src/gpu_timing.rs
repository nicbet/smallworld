//! GPU timestamp queries with per-pass rolling averages.

const EMA_ALPHA: f64 = 0.05;

/// Per-pass GPU timestamps with EMA-smoothed readback.
pub struct GpuTimestamps {
    query_set: wgpu::QuerySet,
    resolve_buf: wgpu::Buffer,
    readback_buf: wgpu::Buffer,
    timestamp_period: f32,
    pass_count: u32,
    averages: Vec<f64>,
    has_data: bool,
}

impl GpuTimestamps {
    /// Creates a timestamp query set for `pass_count` passes (2 queries each).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, pass_count: u32) -> Self {
        let query_count = pass_count * 2;
        let buf_size = (query_count as u64) * size_of::<u64>() as u64;

        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu_timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: query_count,
        });

        let resolve_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp_resolve"),
            size: buf_size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timestamp_readback"),
            size: buf_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let timestamp_period = queue.get_timestamp_period();

        Self {
            query_set,
            resolve_buf,
            readback_buf,
            timestamp_period,
            pass_count,
            averages: vec![0.0; pass_count as usize],
            has_data: false,
        }
    }

    /// Returns timestamp writes for a compute pass at the given pass index.
    pub fn compute_pass_writes(&self, index: u32) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(index * 2),
            end_of_pass_write_index: Some(index * 2 + 1),
        }
    }

    /// Returns timestamp writes for a render pass at the given pass index.
    pub fn render_pass_writes(&self, index: u32) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(index * 2),
            end_of_pass_write_index: Some(index * 2 + 1),
        }
    }

    /// Resolves the query set and copies results to the readback buffer.
    /// Call after all passes are recorded but before submitting.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        let query_count = self.pass_count * 2;
        let buf_size = (query_count as u64) * size_of::<u64>() as u64;
        encoder.resolve_query_set(&self.query_set, 0..query_count, &self.resolve_buf, 0);
        encoder.copy_buffer_to_buffer(&self.resolve_buf, 0, &self.readback_buf, 0, buf_size);
    }

    /// Reads timestamp results from the previous frame and updates rolling averages.
    /// Call at the start of the frame, before encoding new commands.
    pub fn read_results(&mut self, device: &wgpu::Device) {
        if !self.has_data {
            self.has_data = true;
            return;
        }

        self.readback_buf
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device lost during timestamp readback");

        {
            let data = self
                .readback_buf
                .slice(..)
                .get_mapped_range()
                .expect("timestamp readback buffer not mapped");
            let ticks: &[u64] = bytemuck::cast_slice(&data);

            for i in 0..self.pass_count as usize {
                let begin = ticks[i * 2];
                let end = ticks[i * 2 + 1];
                let duration_ns = (end.wrapping_sub(begin)) as f64 * self.timestamp_period as f64;
                let duration_ms = duration_ns / 1_000_000.0;
                self.averages[i] = self.averages[i] * (1.0 - EMA_ALPHA) + duration_ms * EMA_ALPHA;
            }
        }

        self.readback_buf.unmap();
    }

    /// Current EMA-smoothed per-pass durations in milliseconds.
    pub fn averages(&self) -> &[f64] {
        &self.averages
    }
}
