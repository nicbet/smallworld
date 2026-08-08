//! GPU context: adapter selection, device/queue creation, capability probing.
//!
//! The engine owns the wgpu state so that downstream subsystems (the raymarcher, the
//! brick pool allocator) can share one device without the viewer needing to know about
//! wgpu internals. The viewer passes a surface reference at creation so the adapter is
//! guaranteed compatible with its window.

/// GPU features and limits probed at boot. Read-only after construction.
///
/// Pipeline compilation (A.4) branches on these flags to select shader variants.
/// The fallback path (SVO software traversal, indirect draw, LDR output) must always
/// exist on hardware that lacks the gated features.
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// `TIMESTAMP_QUERY` — GPU profiling timestamps.
    pub timestamp_query: bool,
    /// `EXPERIMENTAL_RAY_QUERY` — hardware ray tracing queries.
    pub ray_query: bool,
    /// `EXPERIMENTAL_MESH_SHADER` — mesh/task shader pipeline.
    pub mesh_shader: bool,
    /// `SHADER_I16` — 16-bit integer support in shaders.
    pub shader_i16: bool,
    /// `SHADER_F16` — 16-bit float support in shaders.
    pub shader_f16: bool,
    /// `SHADER_INT64_ATOMIC_ALL_OPS` — 64-bit atomic operations.
    pub int64_atomics: bool,
    /// `SUBGROUP` — subgroup/wave intrinsics.
    pub subgroups: bool,

    /// Supported HDR color spaces for `Rgba16Float` surfaces.
    pub hdr_color_spaces: wgpu::SurfaceColorSpaces,

    /// Largest single `wgpu::Buffer` the device can allocate, in MB.
    /// On Vulkan/DX12 this can exceed the per-binding cap;
    pub max_buffer_mb: u32,
    /// Largest storage-buffer binding visible to a single shader dispatch, in MB.
    /// Brick pool and SVO sizes must stay within this limit.
    pub max_ssbo_binding_mb: u32,
    /// `min_uniform_buffer_offset_alignment` in bytes.
    pub min_ubo_align: u32,
    /// `max_texture_dimension_2d`.
    pub max_texture_dim: u32,

    /// Human-readable adapter name.
    pub adapter_name: String,
    /// Graphics backend (Vulkan, Metal, DX12, …).
    pub backend: wgpu::Backend,
}

impl Capabilities {
    fn probe(
        adapter: &wgpu::Adapter,
        surface: Option<(&wgpu::Surface<'_>, &wgpu::Adapter)>,
    ) -> Self {
        let features = adapter.features();
        let limits = adapter.limits();
        let info = adapter.get_info();

        let hdr_color_spaces = match surface {
            Some((surf, adpt)) => {
                let caps = surf.get_capabilities(adpt);
                caps.color_spaces(wgpu::TextureFormat::Rgba16Float)
            }
            None => wgpu::SurfaceColorSpaces::empty(),
        };

        Self {
            timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            ray_query: features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY),
            mesh_shader: features.contains(wgpu::Features::EXPERIMENTAL_MESH_SHADER),
            shader_i16: features.contains(wgpu::Features::SHADER_I16),
            shader_f16: features.contains(wgpu::Features::SHADER_F16),
            int64_atomics: features.contains(wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS),
            subgroups: features.contains(wgpu::Features::SUBGROUP),
            hdr_color_spaces,
            max_buffer_mb: (limits.max_buffer_size / (1024 * 1024)) as u32,
            max_ssbo_binding_mb: (limits.max_storage_buffer_binding_size / (1024 * 1024)) as u32,
            min_ubo_align: limits.min_uniform_buffer_offset_alignment,
            max_texture_dim: limits.max_texture_dimension_2d,
            adapter_name: info.name.clone(),
            backend: info.backend,
        }
    }

    fn log_summary(&self) {
        let flags: Vec<&str> = [
            (self.timestamp_query, "timestamps"),
            (self.ray_query, "ray_query"),
            (self.mesh_shader, "mesh_shader"),
            (self.shader_i16, "i16"),
            (self.shader_f16, "f16"),
            (self.int64_atomics, "int64_atomics"),
            (self.subgroups, "subgroups"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect();

        let hdr_label = if self.hdr_color_spaces.contains(wgpu::SurfaceColorSpaces::EXTENDED_SRGB_LINEAR) {
            "ExtendedSrgbLinear"
        } else if self.hdr_color_spaces.contains(wgpu::SurfaceColorSpaces::BT2100_PQ) {
            "HDR10 (PQ)"
        } else if self.hdr_color_spaces.contains(wgpu::SurfaceColorSpaces::BT2100_HLG) {
            "HLG"
        } else if self.hdr_color_spaces.contains(wgpu::SurfaceColorSpaces::DISPLAY_P3) {
            "Display P3"
        } else {
            "none"
        };

        log::info!(
            "capabilities: features=[{}] hdr={} buffer={}MB ssbo_binding={}MB ubo_align={} max_tex={}",
            flags.join(", "),
            hdr_label,
            self.max_buffer_mb,
            self.max_ssbo_binding_mb,
            self.min_ubo_align,
            self.max_texture_dim,
        );
    }
}

/// Everything needed to issue GPU work.
pub struct GpuContext {
    /// The wgpu instance that created the adapter.
    pub instance: wgpu::Instance,
    /// The selected physical adapter.
    pub adapter: wgpu::Adapter,
    /// The logical device.
    pub device: wgpu::Device,
    /// The command submission queue.
    pub queue: wgpu::Queue,
    /// Probed hardware capabilities — read-only after boot.
    pub caps: Capabilities,
}

impl GpuContext {
    /// Creates a context compatible with `surface`.
    ///
    /// Prefers a high-performance (discrete) adapter. Panics if no suitable adapter is
    /// found — there is no meaningful fallback for a GPU engine.
    pub async fn new(instance: wgpu::Instance, surface: &wgpu::Surface<'_>) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(surface),
                force_fallback_adapter: false,
                apply_limit_buckets: true,
            })
            .await
            .expect("no compatible GPU adapter found");

        log::info!(
            "adapter: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        let caps = Capabilities::probe(&adapter, Some((surface, &adapter)));

        let (required_features, experimental_features) = Self::negotiate_features(&adapter);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("smallworld"),
                required_features,
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features,
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        caps.log_summary();

        Self {
            instance,
            adapter,
            device,
            queue,
            caps,
        }
    }

    /// Creates a context for the best available adapter without a surface.
    ///
    /// Used by the `--info` path to probe adapter capabilities in headless CI.
    pub async fn headless(instance: wgpu::Instance) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: true,
            })
            .await
            .expect("no GPU adapter found");

        let caps = Capabilities::probe(&adapter, None);

        let (required_features, experimental_features) = Self::negotiate_features(&adapter);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("smallworld-headless"),
                required_features,
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features,
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        caps.log_summary();

        Self {
            instance,
            adapter,
            device,
            queue,
            caps,
        }
    }

    /// Adapter metadata for display in the debug overlay.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Whether the device supports GPU timestamp queries.
    pub fn supports_timestamps(&self) -> bool {
        self.caps.timestamp_query
    }

    fn negotiate_features(
        adapter: &wgpu::Adapter,
    ) -> (wgpu::Features, wgpu::ExperimentalFeatures) {
        let available = adapter.features();
        let mut features = wgpu::Features::empty();

        let probed = [
            wgpu::Features::TIMESTAMP_QUERY,
            wgpu::Features::EXPERIMENTAL_RAY_QUERY,
            wgpu::Features::EXPERIMENTAL_MESH_SHADER,
            wgpu::Features::SHADER_I16,
            wgpu::Features::SHADER_F16,
            wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS,
            wgpu::Features::SUBGROUP,
        ];

        for feature in probed {
            if available.contains(feature) {
                features |= feature;
            }
        }

        let experimental_mask = wgpu::Features::all_experimental_mask();
        let experimental_token = if features.intersects(experimental_mask) {
            // SAFETY: we opt into experimental wgpu features (ray query, mesh shaders)
            // whose APIs may have UB-containing bugs. We accept the risk.
            #[allow(unsafe_code)]
            unsafe { wgpu::ExperimentalFeatures::enabled() }
        } else {
            wgpu::ExperimentalFeatures::disabled()
        };

        (features, experimental_token)
    }

    /// Convenience: create a [`wgpu::Instance`] with the platform default backends.
    pub fn create_instance() -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        })
    }

    /// Configure (or reconfigure) a surface for the current device.
    pub fn configure_surface(
        &self,
        surface: &wgpu::Surface<'_>,
        width: u32,
        height: u32,
    ) -> wgpu::SurfaceConfiguration {
        let caps = surface.get_capabilities(&self.adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::default(),
        };
        surface.configure(&self.device, &config);
        config
    }
}
