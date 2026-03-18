//! GPU device and visual effects — flames, chrome, heat glow, starburst.
//!
//! Each effect is a separate render pass that composites with the egui layer.
//! All share the same wgpu::Device from SharedGpu.

pub mod chrome;
pub mod flames;
pub mod heat_glow;
pub mod starburst;

/// GPU device error types
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no suitable GPU adapter found")]
    NoAdapter,

    #[error("failed to request device: {0}")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),

    #[error("failed to create surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
}

/// Shared GPU resources created once and passed to all rendering subsystems.
///
/// The device and queue are shared between:
/// - egui_wgpu::Renderer (panel UI)
/// - Custom shader pipelines (flames, chrome, heat glow, starburst)
/// - Burn WgpuDevice (inference, future)
pub struct SharedGpu {
    /// wgpu instance (used for creating surfaces)
    pub instance: wgpu::Instance,
    /// Physical adapter info
    pub adapter: wgpu::Adapter,
    /// Logical device handle
    pub device: wgpu::Device,
    /// Command submission queue
    pub queue: wgpu::Queue,
}

impl SharedGpu {
    /// Create shared GPU resources.
    ///
    /// Tries Vulkan first (primary backend for Hyprland/Wayland), falls back
    /// to GL if Vulkan isn't available (CI, headless, older hardware).
    ///
    /// # Errors
    /// Returns `GpuError::NoAdapter` if no GPU is available at all.
    pub async fn new() -> Result<Self, GpuError> {
        // Try Vulkan first, then GL fallback
        let backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .ok_or(GpuError::NoAdapter)?;

        tracing::info!(
            backend = ?adapter.get_info().backend,
            name = %adapter.get_info().name,
            "GPU adapter selected"
        );

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("hotbar"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    /// Get adapter info for diagnostics.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Get the preferred texture format for a surface.
    ///
    /// Used when configuring the swap chain for the panel window.
    pub fn preferred_format(&self, surface: &wgpu::Surface) -> wgpu::TextureFormat {
        let caps = surface.get_capabilities(&self.adapter);
        caps.formats
            .first()
            .copied()
            .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb)
    }
}

/// Per-frame parameters for all GPU effects.
pub struct FrameParams {
    /// Activity level: events per second, clamped to 0.0..1.0
    pub heat_intensity: f32,
    /// Panel width in pixels
    pub width: u32,
    /// Panel height in pixels
    pub height: u32,
    /// Delta time in seconds
    pub dt: f32,
    /// Currently selected spinner index
    pub selected_index: usize,
    /// Y position of selected slot center (screen-space pixels)
    pub selected_y: f32,
    /// Scan-line wavelength in pixels (from reveal animation state)
    pub scanline_lambda: f32,
    /// Scan-line scroll rate (from reveal animation state)
    pub scanline_omega: f32,
    /// Scissor clipping rect `[x, y, width, height]` for reveal animation.
    /// Full surface `[0, 0, width, height]` when no clipping is active.
    pub scissor: [u32; 4],
}

/// All GPU effects, initialized once and updated each frame.
pub struct GpuEffects {
    /// Brushed metal background (pass 1)
    pub chrome: chrome::ChromePass,
    /// Edge glow driven by activity (pass 2)
    pub heat_glow: heat_glow::HeatGlowPass,
    /// Flame particles along edges (pass 3)
    pub flames: flames::FlamePass,
    /// Selection explosion effect (pass 5)
    pub starburst: starburst::StarburstPass,
    /// Elapsed time since startup (seconds)
    time: f32,
    /// Starburst trigger intensity (decays from 1.0 to 0.0 over 0.3s)
    starburst_intensity: f32,
    /// Previous selected index (to detect changes and trigger starburst)
    prev_selected: usize,
}

impl GpuEffects {
    /// Create all effect pipelines.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            chrome: chrome::ChromePass::new(device, format),
            heat_glow: heat_glow::HeatGlowPass::new(device, format),
            flames: flames::FlamePass::new(device, format, width, height),
            starburst: starburst::StarburstPass::new(device, format),
            time: 0.0,
            starburst_intensity: 0.0,
            prev_selected: 0,
        }
    }

    /// Render all pre-egui passes (chrome, heat glow, flames).
    ///
    /// Returns hot-spot Y positions from the fire automaton (for cinder
    /// ember ejection). Caller should feed up to 2 per frame to
    /// `CinderSystem::spawn_at()`.
    pub fn render_before_egui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        queue: &wgpu::Queue,
        params: &FrameParams,
    ) -> Vec<f32> {
        self.time += params.dt;

        // Detect selection change to trigger starburst
        if params.selected_index != self.prev_selected {
            self.starburst_intensity = 1.0;
            self.prev_selected = params.selected_index;
        }
        self.starburst_intensity = (self.starburst_intensity - params.dt / 0.3).max(0.0);

        // Pass 1: Chrome background (with scan-lines from reveal state)
        {
            crate::dev_trace_span!("chrome_pass");
            self.chrome.render(
                encoder,
                view,
                queue,
                params.width,
                params.height,
                self.time,
                params.scanline_lambda,
                params.scanline_omega,
                params.scissor,
            );
        }

        // Pass 2: Heat glow border (fire automaton step, then render)
        let hot_spots;
        {
            crate::dev_trace_span!("heat_glow_pass");
            self.heat_glow.update_fire(queue, params.heat_intensity, params.height);
            hot_spots = self.heat_glow.hot_spots(0.7, params.height);
            {
                crate::dev_trace_span!("heat_glow_encode");
                self.heat_glow.render(
                    encoder,
                    view,
                    queue,
                    params.width,
                    params.height,
                    params.heat_intensity,
                    self.time,
                    params.scissor,
                );
            }
        }

        // Pass 3: Flame particles
        {
            crate::dev_trace_span!("flames_pass");
            self.flames.update(
                queue,
                params.heat_intensity,
                params.dt,
                params.width,
                params.height,
            );
            self.flames.render(encoder, view, params.scissor);
        }

        hot_spots
    }

    /// Render all post-egui passes (starburst).
    pub fn render_after_egui(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        queue: &wgpu::Queue,
        params: &FrameParams,
    ) {
        // Pass 5: Starburst (only when active)
        crate::dev_trace_span!("starburst_pass");
        if self.starburst_intensity > 0.01 {
            let center_y_normalized = if params.height > 0 {
                params.selected_y / params.height as f32
            } else {
                0.5
            };
            self.starburst.render(
                encoder,
                view,
                queue,
                params.width,
                params.height,
                center_y_normalized,
                self.starburst_intensity,
                self.time,
                params.scissor,
            );
        }
    }

    /// Resize all effect buffers (called on surface reconfigure).
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.flames.resize(device, width, height);
    }

    /// Current starburst intensity (for diagnostics/testing).
    pub fn starburst_intensity(&self) -> f32 {
        self.starburst_intensity
    }

    /// Elapsed time since creation (for diagnostics).
    pub fn elapsed_time(&self) -> f32 {
        self.time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // GPU tests require actual hardware; skip gracefully on headless/CI
    // by checking for adapter availability.

    #[tokio::test]
    async fn shared_gpu_creation() {
        // This test validates the construction path. On systems without
        // a GPU (CI), the test succeeds by verifying the error type.
        match SharedGpu::new().await {
            Ok(gpu) => {
                let info = gpu.adapter_info();
                // Should have a name and backend
                assert!(!info.name.is_empty());
                tracing::info!(
                    name = %info.name,
                    backend = ?info.backend,
                    "GPU test: adapter available"
                );
            }
            Err(GpuError::NoAdapter) => {
                // Expected on headless systems — not a failure
                tracing::info!("GPU test: no adapter available (headless/CI)");
            }
            Err(e) => {
                panic!("unexpected GPU error: {e}");
            }
        }
    }
}
