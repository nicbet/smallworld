use glam::Vec3Swizzles;

use crate::scenes::Preset;

pub struct BenchConfig {
    pub preset: Preset,
    pub duration_secs: f32,
}

pub struct BenchState {
    pub config: BenchConfig,
    pub start: std::time::Instant,
    pub samples: Vec<BenchSample>,
    pub orbit_angle: f32,
    pub orbit_radius: f32,
    pub orbit_height: f32,
    pub orbit_center: glam::Vec3,
}

#[derive(Clone, Copy)]
pub struct BenchSample {
    pub dt_ms: f32,
    pub cpu_ms: f32,
    pub gpu_compute_ms: f64,
    pub gpu_blit_ms: f64,
    pub gpu_egui_ms: f64,
}

impl BenchState {
    pub fn new(config: BenchConfig, preset: Preset) -> Self {
        let grid_dims = preset.grid_dims();
        let world_min = preset.world_min();
        let extent = glam::Vec3::new(
            grid_dims[0] as f32,
            grid_dims[1] as f32,
            grid_dims[2] as f32,
        ) * smallworld_engine::brick_pool::VOXEL_SCALE
            * smallworld_engine::brick_pool::BRICK_EDGE as f32;
        let center = world_min + extent * 0.5;
        let radius = extent.x.max(extent.z) * 0.6;
        let height = center.y + extent.y * 0.6;

        Self {
            config,
            start: std::time::Instant::now(),
            samples: Vec::with_capacity(2000),
            orbit_angle: 0.0,
            orbit_radius: radius.max(5.0),
            orbit_height: height.max(3.0),
            orbit_center: glam::Vec3::new(center.x, 0.0, center.z),
        }
    }

    pub fn elapsed_secs(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }

    pub fn is_done(&self) -> bool {
        self.elapsed_secs() >= self.config.duration_secs
    }

    pub fn advance_orbit(&mut self, dt: f32, camera: &mut smallworld_engine::camera::FreeCamera) {
        let rate = std::f32::consts::TAU / self.config.duration_secs;
        self.orbit_angle += rate * dt;

        let x = self.orbit_center.x + self.orbit_radius * self.orbit_angle.cos();
        let z = self.orbit_center.z + self.orbit_radius * self.orbit_angle.sin();
        camera.position = glam::Vec3::new(x, self.orbit_height, z);

        let to_center = self.orbit_center - camera.position;
        camera.yaw = to_center.z.atan2(to_center.x);
        camera.pitch = (-to_center.y).atan2(to_center.xz().length());
    }

    pub fn push_sample(&mut self, sample: BenchSample) {
        self.samples.push(sample);
    }

    pub fn print_report(&self, brick_count: u32, pool_capacity: u32, instance_count: u32) {
        let n = self.samples.len();
        if n == 0 {
            println!("no frames recorded");
            return;
        }

        let elapsed = self.elapsed_secs();
        let preset_name = self.config.preset.label();

        let dts: Vec<f32> = self.samples.iter().map(|s| s.dt_ms).collect();
        let cpus: Vec<f32> = self.samples.iter().map(|s| s.cpu_ms).collect();
        let gpus: Vec<f32> = self
            .samples
            .iter()
            .map(|s| (s.gpu_compute_ms + s.gpu_blit_ms + s.gpu_egui_ms) as f32)
            .collect();
        let fps: Vec<f32> = self
            .samples
            .iter()
            .map(|s| if s.dt_ms > 0.0 { 1000.0 / s.dt_ms } else { 0.0 })
            .collect();

        println!();
        println!("smallworld bench — {preset_name}, {elapsed:.1}s, {n} frames");
        println!("──────────────────────────────────────────────────");
        println!(
            "            {:>8} {:>8} {:>8} {:>8}",
            "min", "avg", "max", "p99"
        );
        print_stat("dt", &dts, "ms");
        print_stat("cpu", &cpus, "ms");
        print_stat("gpu", &gpus, "ms");
        print_stat("fps", &fps, "");
        println!();
        println!(
            "  bricks: {} / {}    instances: {}",
            brick_count, pool_capacity, instance_count
        );
        println!("──────────────────────────────────────────────────");
        println!();
    }
}

fn print_stat(label: &str, values: &[f32], unit: &str) {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let avg = sorted.iter().sum::<f32>() / sorted.len() as f32;
    let p99_idx = ((sorted.len() as f32) * 0.99) as usize;
    let p99 = sorted[p99_idx.min(sorted.len() - 1)];

    println!("  {label:<6} {min:>8.2} {avg:>8.2} {max:>8.2} {p99:>8.2}  {unit}");
}

pub fn parse_args() -> Option<BenchConfig> {
    let args: Vec<String> = std::env::args().collect();
    let bench_idx = args.iter().position(|a| a == "--bench")?;

    let mut preset = Preset::Default;
    let mut duration_secs = 20.0_f32;

    let mut i = bench_idx + 1;
    while i < args.len() {
        if args[i] == "--duration" {
            i += 1;
            if i < args.len() {
                duration_secs = args[i].parse().unwrap_or(20.0);
            }
        } else {
            for &p in Preset::ALL {
                if p.label().eq_ignore_ascii_case(&args[i])
                    || p.label().replace(' ', "").eq_ignore_ascii_case(&args[i])
                {
                    preset = p;
                    break;
                }
            }
        }
        i += 1;
    }

    Some(BenchConfig {
        preset,
        duration_secs,
    })
}
