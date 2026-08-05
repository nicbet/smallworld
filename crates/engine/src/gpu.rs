//! GPU context: adapter selection, device/queue creation.
//!
//! The engine owns the wgpu state so that downstream subsystems (the raymarcher, the
//! brick pool allocator) can share one device without the viewer needing to know about
//! wgpu internals. The viewer passes a surface reference at creation so the adapter is
//! guaranteed compatible with its window.

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

        let required_features = Self::negotiate_features(&adapter);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("smallworld"),
                required_features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        Self {
            instance,
            adapter,
            device,
            queue,
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

        let required_features = Self::negotiate_features(&adapter);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("smallworld-headless"),
                required_features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        Self {
            instance,
            adapter,
            device,
            queue,
        }
    }

    /// Adapter metadata for display in the debug overlay.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Whether the device supports GPU timestamp queries.
    pub fn supports_timestamps(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
    }

    fn negotiate_features(adapter: &wgpu::Adapter) -> wgpu::Features {
        let available = adapter.features();
        let mut features = wgpu::Features::empty();
        if available.contains(wgpu::Features::TIMESTAMP_QUERY) {
            features |= wgpu::Features::TIMESTAMP_QUERY;
            log::info!("GPU timestamps enabled");
        } else {
            log::warn!("GPU timestamps not supported by this adapter");
        }
        features
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
