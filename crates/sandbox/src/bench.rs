use crate::scenes::Preset;

pub struct BenchConfig {
    pub preset: Preset,
    pub duration_secs: f32,
}

pub struct BenchState {
    pub config: BenchConfig,
    pub start: std::time::Instant,
    pub samples: Vec<BenchSample>,
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
    pub fn new(config: BenchConfig) -> Self {
        Self {
            config,
            start: std::time::Instant::now(),
            samples: Vec::with_capacity(2000),
        }
    }

    pub fn elapsed_secs(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }

    pub fn is_done(&self) -> bool {
        self.elapsed_secs() >= self.config.duration_secs
    }

    pub fn advance_camera(&self, camera: &mut smallworld_engine::camera::FreeCamera) {
        let t = self.elapsed_secs() / self.config.duration_secs;
        let (pos, yaw, pitch) = self.config.preset.camera_path(t);
        camera.position = pos;
        camera.yaw = yaw;
        camera.pitch = pitch;
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

        let dt_s = stat(&dts);
        let cpu_s = stat(&cpus);
        let gpu_s = stat(&gpus);
        let fps_s = stat(&fps);
        println!(
            "{{\"preset\":\"{preset_name}\",\"duration_secs\":{elapsed:.2},\"frames\":{n},\
             \"dt\":{{\"min\":{:.2},\"avg\":{:.2},\"max\":{:.2},\"p99\":{:.2}}},\
             \"cpu\":{{\"min\":{:.2},\"avg\":{:.2},\"max\":{:.2},\"p99\":{:.2}}},\
             \"gpu\":{{\"min\":{:.2},\"avg\":{:.2},\"max\":{:.2},\"p99\":{:.2}}},\
             \"fps\":{{\"min\":{:.2},\"avg\":{:.2},\"max\":{:.2},\"p99\":{:.2}}},\
             \"bricks\":{brick_count},\"pool_capacity\":{pool_capacity},\"instances\":{instance_count}}}",
            dt_s.0,
            dt_s.1,
            dt_s.2,
            dt_s.3,
            cpu_s.0,
            cpu_s.1,
            cpu_s.2,
            cpu_s.3,
            gpu_s.0,
            gpu_s.1,
            gpu_s.2,
            gpu_s.3,
            fps_s.0,
            fps_s.1,
            fps_s.2,
            fps_s.3,
        );
    }
}

fn print_stat(label: &str, values: &[f32], unit: &str) {
    let (min, avg, max, p99) = stat(values);
    println!("  {label:<6} {min:>8.2} {avg:>8.2} {max:>8.2} {p99:>8.2}  {unit}");
}

fn stat(values: &[f32]) -> (f32, f32, f32, f32) {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let avg = sorted.iter().sum::<f32>() / sorted.len() as f32;
    let p99_idx = ((sorted.len() as f32) * 0.99) as usize;
    let p99 = sorted[p99_idx.min(sorted.len() - 1)];
    (min, avg, max, p99)
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
            let normalized = args[i].replace(['-', ' '], "");
            for &p in Preset::ALL {
                let label_norm = p.label().replace(' ', "");
                if label_norm.eq_ignore_ascii_case(&normalized) {
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
