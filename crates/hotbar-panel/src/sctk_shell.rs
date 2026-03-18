//! SCTK layer-shell surface creation and event loop.
//!
//! Creates a Wayland layer-shell surface anchored to the right edge of the screen,
//! bridges SCTK input events to egui `RawInput`, and drives the wgpu/egui render loop.

use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle, DisplayHandle, WindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::client::{
        protocol::{
            wl_keyboard::{self, WlKeyboard},
            wl_output::WlOutput,
            wl_pointer::{self, WlPointer},
            wl_seat::WlSeat,
            wl_surface::WlSurface,
        },
        Connection, Dispatch, QueueHandle,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    },
    shell::WaylandSurface,
    shm::{Shm, ShmHandler},
};
use wayland_client::protocol::wl_keyboard::KeyState;
use wayland_client::{Proxy, WEnum};

use crate::anim::{AgentShake, BurnInMitigation, PanelReveal, RevealPhase};
use crate::gpu::{SharedGpu, GpuEffects, FrameParams};
use crate::theme;

/// Type alias for UI callback function
type UiCallback = Box<dyn FnMut(&egui::Context)>;

/// Shell error types
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("wayland connection failed: {0}")]
    Connection(#[from] wayland_client::ConnectError),

    #[error("wayland global error: {0}")]
    Global(#[from] wayland_client::globals::GlobalError),

    #[error("bind error: {0}")]
    Bind(String),

    #[error("layer shell not available")]
    NoLayerShell,

    #[error("no output available")]
    NoOutput,

    #[error("calloop error: {0}")]
    Calloop(String),

    #[error("GPU error: {0}")]
    Gpu(#[from] crate::gpu::GpuError),

    #[error("wgpu surface error: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
}

/// Visibility state of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelVisibility {
    Visible,
    Hidden,
}

/// Panel configuration.
pub struct PanelConfig {
    /// Panel width in pixels
    pub width: u32,
    /// Panel margin from screen edge
    pub margin: i32,
    /// Anchor side (TOP|RIGHT|BOTTOM = right edge panel)
    pub anchor: Anchor,
    /// Layer shell layer (Overlay = above all windows)
    pub layer: Layer,
    /// Keyboard interactivity mode
    pub keyboard_interactivity: KeyboardInteractivity,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            width: theme::PANEL_WIDTH as u32,
            margin: theme::PANEL_MARGIN as i32,
            anchor: Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM,
            layer: Layer::Overlay,
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
        }
    }
}

/// Wrapper around SCTK wl_surface to implement raw-window-handle traits.
///
/// wgpu needs these handles to create a rendering surface from the Wayland surface.
#[derive(Clone)]
struct WaylandWindow {
    surface: *mut std::ffi::c_void,
    display: *mut std::ffi::c_void,
}

// Safety: Wayland objects are thread-safe (they're protocol proxies).
unsafe impl Send for WaylandWindow {}
unsafe impl Sync for WaylandWindow {}

impl HasWindowHandle for WaylandWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = RawWindowHandle::Wayland(WaylandWindowHandle::new(
            std::ptr::NonNull::new(self.surface)
                .expect("null wl_surface pointer"),
        ));
        // Safety: the surface pointer is valid for the lifetime of the LayerSurface
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for WaylandWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            std::ptr::NonNull::new(self.display)
                .expect("null wl_display pointer"),
        ));
        // Safety: the display pointer is valid for the lifetime of the Connection
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

/// Main application state for the SCTK event loop.
pub struct HotbarShell {
    // ── Wayland state ──
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    shm_state: Shm,
    layer_shell: LayerShell,

    // ── Surface ──
    layer_surface: Option<LayerSurface>,
    surface_configured: bool,
    width: u32,
    height: u32,
    display_ptr: *mut std::ffi::c_void,

    // ── Input ──
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    pointer_pos: egui::Pos2,
    modifiers: egui::Modifiers,

    // ── Render ──
    gpu: Option<SharedGpu>,
    wgpu_surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    egui_ctx: egui::Context,
    egui_renderer: Option<egui_wgpu::Renderer>,
    egui_input: egui::RawInput,

    // ── GPU effects ──
    /// GPU visual effects (flames, chrome, heat glow, starburst)
    gpu_effects: Option<GpuEffects>,
    /// Current heat intensity from daemon activity tracker (0.0..1.0)
    heat_intensity: f32,
    /// Current selected spinner index
    selected_index: usize,
    /// Last frame timestamp for accurate dt calculation
    last_frame_time: std::time::Instant,

    // ── Animation ──
    /// Panel reveal state machine (3-phase entrance animation)
    panel_reveal: PanelReveal,
    /// OLED burn-in mitigation (periodic subpixel content shift)
    burn_in: BurnInMitigation,
    /// Agent shake (horizontal oscillation on heat spike)
    agent_shake: AgentShake,

    // ── App state ──
    visibility: PanelVisibility,
    config: PanelConfig,
    exit_requested: bool,

    // ── Callbacks ──
    /// Called each frame with the egui context to draw the UI.
    /// Set via `set_ui_callback` before running the event loop.
    ui_callback: Option<UiCallback>,
}

impl HotbarShell {
    /// Create a new shell by connecting to the Wayland compositor.
    pub fn new(config: PanelConfig) -> Result<(Self, EventLoop<'static, Self>, QueueHandle<Self>), ShellError> {
        let conn = Connection::connect_to_env()?;
        let (globals, event_queue) = wayland_client::globals::registry_queue_init(&conn)?;
        let qh = event_queue.handle();

        let registry_state = RegistryState::new(&globals);
        let seat_state = SeatState::new(&globals, &qh);
        let output_state = OutputState::new(&globals, &qh);
        let compositor_state = CompositorState::bind(&globals, &qh)
            .map_err(|e| ShellError::Bind(e.to_string()))?;
        let shm_state = Shm::bind(&globals, &qh)
            .map_err(|e| ShellError::Bind(e.to_string()))?;
        let layer_shell = LayerShell::bind(&globals, &qh)
            .map_err(|e| ShellError::Bind(e.to_string()))?;

        let egui_ctx = egui::Context::default();

        let event_loop: EventLoop<Self> = EventLoop::try_new()
            .map_err(|e| ShellError::Calloop(e.to_string()))?;

        // For wayland-client 0.31, we can't easily get the display pointer before the connection
        // is consumed. wgpu can work with a null display pointer and infer it from the surface.
        let display_ptr = std::ptr::null_mut();

        let wayland_source = WaylandSource::new(conn, event_queue);
        event_loop
            .handle()
            .insert_source(wayland_source, |_, _, _| Ok(0usize))
            .map_err(|e| ShellError::Calloop(e.to_string()))?;

        let shell = Self {
            registry_state,
            seat_state,
            output_state,
            compositor_state,
            shm_state,
            layer_shell,
            layer_surface: None,
            surface_configured: false,
            width: config.width,
            height: 0,
            display_ptr,
            pointer: None,
            keyboard: None,
            pointer_pos: egui::Pos2::ZERO,
            modifiers: egui::Modifiers::NONE,
            gpu: None,
            wgpu_surface: None,
            surface_config: None,
            egui_ctx,
            egui_renderer: None,
            egui_input: egui::RawInput::default(),
            gpu_effects: None,
            heat_intensity: 0.0,
            selected_index: 0,
            last_frame_time: std::time::Instant::now(),
            panel_reveal: PanelReveal::new(config.width as f32),
            burn_in: BurnInMitigation::new(),
            agent_shake: AgentShake::new(),
            visibility: PanelVisibility::Visible,
            config,
            exit_requested: false,
            ui_callback: None,
        };

        Ok((shell, event_loop, qh))
    }

    /// Create the layer surface and show the panel.
    pub fn create_surface(&mut self, qh: &QueueHandle<Self>) {
        tracing::debug!("creating layer-shell surface");
        let wl_surface = self.compositor_state.create_surface(qh);
        let layer_surface = self.layer_shell.create_layer_surface(
            qh,
            wl_surface,
            self.config.layer,
            Some("hotbar"),
            None, // output — None = compositor picks
        );

        layer_surface.set_anchor(self.config.anchor);
        layer_surface.set_size(self.config.width, 0); // 0 height = fill anchor
        layer_surface.set_margin(
            self.config.margin,
            self.config.margin,
            self.config.margin,
            0,
        );
        layer_surface.set_keyboard_interactivity(self.config.keyboard_interactivity);
        layer_surface.set_exclusive_zone(-1); // Don't push other surfaces

        layer_surface.wl_surface().commit();
        self.layer_surface = Some(layer_surface);
    }

    /// Initialize GPU resources and egui renderer once the surface is configured.
    fn init_gpu(&mut self) -> Result<(), ShellError> {
        let _span = tracing::debug_span!("init_gpu").entered();
        let layer_surface = self.layer_surface.as_ref()
            .expect("init_gpu called before surface created");

        let gpu = pollster::block_on(SharedGpu::new())?;

        let wl_surface = layer_surface.wl_surface();
        // Get the native pointer from the surface
        let surface_ptr = wl_surface.id().protocol_id() as usize as *mut std::ffi::c_void;

        let window_handle = Box::new(WaylandWindow {
            surface: surface_ptr,
            display: self.display_ptr,
        });

        // Safety: We leak the box to get a 'static reference, which is required by wgpu.
        // The surface will live as long as the window, so this is safe.
        let window_handle: &'static WaylandWindow = Box::leak(window_handle);

        let wgpu_surface = gpu.instance.create_surface(
            wgpu::SurfaceTarget::from(window_handle)
        )?;

        let format = gpu.preferred_format(&wgpu_surface);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: self.width,
            height: self.height,
            present_mode: wgpu::PresentMode::Mailbox,
            alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        wgpu_surface.configure(&gpu.device, &surface_config);

        let egui_renderer = egui_wgpu::Renderer::new(
            &gpu.device,
            format,
            None, // depth format
            1,    // sample count
            false,
        );

        let effects = GpuEffects::new(&gpu.device, format, self.width, self.height);

        self.wgpu_surface = Some(wgpu_surface);
        self.surface_config = Some(surface_config);
        self.egui_renderer = Some(egui_renderer);
        self.gpu_effects = Some(effects);
        self.gpu = Some(gpu);

        tracing::info!(
            width = self.width,
            height = self.height,
            "GPU initialized for panel surface (with effects)"
        );

        Ok(())
    }

    /// Render a frame: GPU effects + egui, composited in 5 passes.
    ///
    /// Pass order:
    /// 1. Chrome background (LoadOp::Clear) — with scan-line modulation
    /// 2. Heat glow edges (LoadOp::Load, additive)
    /// 3. Flame particles (LoadOp::Load, additive)
    /// 4. egui widgets (LoadOp::Load, alpha blend)
    /// 5. Starburst (LoadOp::Load, additive)
    ///
    /// All passes are clipped by a scissor rect during the reveal animation.
    fn render_frame(&mut self) {
        crate::dev_trace_span!("render_frame");

        let Some(gpu) = &self.gpu else { return };
        let Some(surface) = &self.wgpu_surface else { return };
        let Some(renderer) = &mut self.egui_renderer else { return };
        let Some(surface_config) = &self.surface_config else { return };
        let Some(effects) = &mut self.gpu_effects else { return };

        // ── Reveal state machine ──
        let reveal = {
            crate::dev_trace_span!("reveal_update");
            self.panel_reveal.update()
        };
        if reveal.phase == RevealPhase::Hidden {
            return; // Nothing to render
        }

        // ── Measure frame dt (used by shake, GPU effects, transitions) ──
        let frame_start = std::time::Instant::now();
        let dt = frame_start.duration_since(self.last_frame_time).as_secs_f32().clamp(0.001, 0.1);
        self.last_frame_time = frame_start;

        // ── Agent shake (horizontal oscillation on heat spike) ──
        let heat_for_shake = reveal.heat_override.unwrap_or(self.heat_intensity);
        let shake_offset = self.agent_shake.update(dt, heat_for_shake);

        // ── Burn-in mitigation (subpixel content shift) ──
        let burn_offset = self.burn_in.update();

        // Prepare egui input with burn-in + shake offset applied to screen rect origin
        let mut input = self.egui_input.take();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(burn_offset.x + shake_offset, burn_offset.y),
            egui::vec2(self.width as f32, self.height as f32),
        ));

        // Apply theme
        theme::apply_theme(&self.egui_ctx);

        // Run egui frame
        let full_output = {
            crate::dev_trace_span!("egui_run");
            self.egui_ctx.run(input, |ctx| {
                if let Some(cb) = &mut self.ui_callback {
                    cb(ctx);
                }
            })
        };

        // Tessellate
        let clipped_primitives = {
            crate::dev_trace_span!("egui_tessellate");
            self.egui_ctx.tessellate(
                full_output.shapes,
                full_output.pixels_per_point,
            )
        };

        // Acquire swapchain frame
        let output_frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&gpu.device, surface_config);
                return;
            }
            Err(e) => {
                tracing::warn!("surface error: {e}");
                return;
            }
        };

        let view = output_frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Update egui textures
        for (id, delta) in &full_output.textures_delta.set {
            renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.width, self.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut encoder = gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("hotbar_encoder"),
            },
        );

        // Compute heat: reveal animation can override daemon-driven heat
        let heat = reveal
            .heat_override
            .unwrap_or(self.heat_intensity)
            .clamp(0.0, 1.0);

        // Compute scissor rect for reveal clipping.
        // Panel is right-anchored: reveal expands from the right edge leftward.
        let scissor = if reveal.width >= self.width as f32 {
            // No clipping needed (Done/idle state, or width >= panel)
            [0, 0, self.width, self.height]
        } else {
            let reveal_px = (reveal.width as u32).min(self.width).max(1);
            let x = self.width.saturating_sub(reveal_px);
            [x, 0, reveal_px, self.height]
        };

        let params = FrameParams {
            heat_intensity: heat,
            width: self.width,
            height: self.height,
            dt,
            selected_index: self.selected_index,
            selected_y: self.height as f32 * 0.5, // approximate center
            scanline_lambda: reveal.scanline_lambda,
            scanline_omega: reveal.scanline_omega,
            scissor,
        };

        // Passes 1-3: Chrome background, heat glow, flame particles
        // Returns fire hot spots for cinder ember ejection (unused until
        // cinder system is wired to the render loop in Phase 5).
        let _hot_spots;
        {
            crate::dev_trace_span!("gpu_before_egui");
            _hot_spots = effects.render_before_egui(&mut encoder, &view, &gpu.queue, &params);
        }

        // Pass 4: egui widgets (LoadOp::Load — composites over GPU effects)
        {
            crate::dev_trace_span!("egui_render");
            renderer.update_buffers(
                &gpu.device,
                &gpu.queue,
                &mut encoder,
                &clipped_primitives,
                &screen_descriptor,
            );

            // wgpu 24: begin_render_pass borrows the encoder. egui-wgpu 0.31
            // requires RenderPass<'static>, so we use forget_lifetime() to erase
            // the borrow and let egui own the pass.
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut render_pass = render_pass.forget_lifetime();
            // Apply reveal scissor to egui pass too
            render_pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
            renderer.render(&mut render_pass, &clipped_primitives, &screen_descriptor);
            drop(render_pass);
        }

        // Pass 5: Starburst (on top of everything)
        {
            crate::dev_trace_span!("gpu_after_egui");
            effects.render_after_egui(&mut encoder, &view, &gpu.queue, &params);
        }

        {
            crate::dev_trace_span!("present");
            gpu.queue.submit(std::iter::once(encoder.finish()));
            output_frame.present();
        }

        // Free textures
        for id in &full_output.textures_delta.free {
            renderer.free_texture(id);
        }

        // Frame budget monitor: warn if CPU-side frame time exceeds 16ms
        let frame_cpu_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
        if frame_cpu_ms > 16.0 {
            tracing::warn!(
                frame_ms = format_args!("{frame_cpu_ms:.1}"),
                "frame budget exceeded (>16ms)"
            );
        }
    }

    /// Set the callback that draws the UI each frame.
    pub fn set_ui_callback(&mut self, callback: impl FnMut(&egui::Context) + 'static) {
        self.ui_callback = Some(Box::new(callback));
    }

    /// Toggle panel visibility with animated reveal.
    pub fn toggle_visibility(&mut self) {
        match self.visibility {
            PanelVisibility::Visible => {
                self.visibility = PanelVisibility::Hidden;
                self.panel_reveal.trigger_close();
            }
            PanelVisibility::Hidden => {
                self.visibility = PanelVisibility::Visible;
                self.panel_reveal.trigger_open();
            }
        }
        tracing::info!(visibility = ?self.visibility, "panel toggled");
    }

    /// Request the event loop to exit.
    pub fn request_exit(&mut self) {
        self.exit_requested = true;
    }

    /// Whether the panel is visible.
    pub fn is_visible(&self) -> bool {
        self.visibility == PanelVisibility::Visible
    }

    /// Whether exit has been requested.
    pub fn should_exit(&self) -> bool {
        self.exit_requested
    }

    /// Panel width.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Panel height.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Set the current heat intensity (driven by daemon activity tracker).
    ///
    /// Value is clamped to 0.0..1.0 in the render loop.
    pub fn set_heat_intensity(&mut self, intensity: f32) {
        self.heat_intensity = intensity;
    }

    /// Set the currently selected spinner index (drives starburst trigger).
    pub fn set_selected_index(&mut self, index: usize) {
        self.selected_index = index;
    }

    /// Access the egui context (for external state setup).
    pub fn egui_ctx(&self) -> &egui::Context {
        &self.egui_ctx
    }
}

// ── SCTK Handler Implementations ────────────────────────────────────

impl CompositorHandler for HotbarShell {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        new_factor: i32,
    ) {
        tracing::debug!(factor = new_factor, "scale factor changed");
        self.egui_input.viewports.entry(egui::ViewportId::ROOT).or_default()
            .native_pixels_per_point = Some(new_factor as f32);
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wayland_client::protocol::wl_output::Transform,
    ) {
        // Not relevant for our panel
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _time: u32,
    ) {
        // Render when visible OR when the reveal animation is still running
        let should_render = self.surface_configured
            && (self.visibility == PanelVisibility::Visible || self.panel_reveal.is_animating());
        if should_render {
            self.render_frame();

            // Request next frame
            if let Some(ls) = &self.layer_surface {
                ls.wl_surface().frame(_qh, ls.wl_surface().clone());
                ls.wl_surface().commit();
            }
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {}

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {}
}

impl OutputHandler for HotbarShell {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: WlOutput,
    ) {}

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: WlOutput,
    ) {}

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: WlOutput,
    ) {}
}

impl LayerShellHandler for HotbarShell {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
    ) {
        self.exit_requested = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let new_width = if configure.new_size.0 > 0 {
            configure.new_size.0
        } else {
            self.config.width
        };
        let new_height = if configure.new_size.1 > 0 {
            configure.new_size.1
        } else {
            1080 // fallback
        };

        let size_changed = new_width != self.width || new_height != self.height;
        self.width = new_width;
        self.height = new_height;

        if !self.surface_configured {
            self.surface_configured = true;
            if let Err(e) = self.init_gpu() {
                tracing::error!("failed to init GPU: {e}");
                self.exit_requested = true;
                return;
            }
            // Request first frame
            layer.wl_surface().frame(_qh, layer.wl_surface().clone());
            layer.wl_surface().commit();
        } else if size_changed {
            // Reconfigure surface
            if let Some(config) = &mut self.surface_config {
                config.width = new_width;
                config.height = new_height;
                if let (Some(gpu), Some(surface)) = (&self.gpu, &self.wgpu_surface) {
                    surface.configure(&gpu.device, config);
                }
                // Resize GPU effects buffers
                if let (Some(gpu), Some(effects)) = (&self.gpu, &mut self.gpu_effects) {
                    effects.resize(&gpu.device, new_width, new_height);
                }
                // Keep reveal aware of new panel dimensions
                self.panel_reveal.set_panel_width(new_width as f32);
            }
        }
    }
}

impl SeatHandler for HotbarShell {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
    ) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let pointer = seat.get_pointer(qh, ());
            self.pointer = Some(pointer);
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let keyboard = seat.get_keyboard(qh, ());
            self.keyboard = Some(keyboard);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(ptr) = self.pointer.take() {
                ptr.release();
            }
        if capability == Capability::Keyboard
            && let Some(kbd) = self.keyboard.take() {
                kbd.release();
            }
    }

    fn remove_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
    ) {}
}

impl PointerHandler for HotbarShell {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.pointer_pos = egui::pos2(
                        event.position.0 as f32,
                        event.position.1 as f32,
                    );
                    self.egui_input.events.push(egui::Event::PointerMoved(self.pointer_pos));
                }
                PointerEventKind::Leave { .. } => {
                    self.egui_input.events.push(egui::Event::PointerGone);
                }
                PointerEventKind::Press { button, .. } => {
                    if let Some(egui_btn) = wayland_button_to_egui(button) {
                        self.egui_input.events.push(egui::Event::PointerButton {
                            pos: self.pointer_pos,
                            button: egui_btn,
                            pressed: true,
                            modifiers: self.modifiers,
                        });
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    if let Some(egui_btn) = wayland_button_to_egui(button) {
                        self.egui_input.events.push(egui::Event::PointerButton {
                            pos: self.pointer_pos,
                            button: egui_btn,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    self.egui_input.events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(
                            horizontal.absolute as f32,
                            vertical.absolute as f32,
                        ),
                        modifiers: self.modifiers,
                    });
                }
            }
        }
    }
}

/// Convert Wayland button codes to egui pointer buttons.
fn wayland_button_to_egui(button: u32) -> Option<egui::PointerButton> {
    match button {
        0x110 => Some(egui::PointerButton::Primary),   // BTN_LEFT
        0x111 => Some(egui::PointerButton::Secondary), // BTN_RIGHT
        0x112 => Some(egui::PointerButton::Middle),    // BTN_MIDDLE
        _ => None,
    }
}

// Keyboard handling via raw wl_keyboard dispatch (SCTK keyboard handler
// requires xkb context setup; we handle keys directly for simplicity).
impl Dispatch<WlKeyboard, ()> for HotbarShell {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                let pressed = key_state == WEnum::Value(KeyState::Pressed);
                // Linux evdev keycodes (offset by 8 from X11 keycodes)
                if let Some(egui_key) = evdev_key_to_egui(key) {
                    state.egui_input.events.push(egui::Event::Key {
                        key: egui_key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: state.modifiers,
                    });
                }
                // Also send as text for search input
                if pressed
                    && let Some(ch) = evdev_key_to_char(key, state.modifiers.shift) {
                        state.egui_input.events.push(egui::Event::Text(ch.to_string()));
                    }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                ..
            } => {
                state.modifiers = egui::Modifiers {
                    alt: mods_depressed & 0x8 != 0,
                    ctrl: mods_depressed & 0x4 != 0,
                    shift: mods_depressed & 0x1 != 0,
                    mac_cmd: false,
                    command: mods_depressed & 0x4 != 0,
                };
            }
            _ => {}
        }
    }
}

// Pointer dispatch (required by SCTK)
impl Dispatch<WlPointer, ()> for HotbarShell {
    fn event(
        _state: &mut Self,
        _proxy: &WlPointer,
        _event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Handled by PointerHandler::pointer_frame
    }
}

impl ShmHandler for HotbarShell {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl ProvidesRegistryState for HotbarShell {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers!(OutputState, SeatState);
}

delegate_compositor!(HotbarShell);
delegate_output!(HotbarShell);
delegate_layer!(HotbarShell);
delegate_seat!(HotbarShell);
delegate_pointer!(HotbarShell);
delegate_shm!(HotbarShell);
delegate_registry!(HotbarShell);

// ── Key Mapping ──────────────────────────────────────────────────────

/// Map evdev keycode to egui key. Subset of keys we care about.
fn evdev_key_to_egui(key: u32) -> Option<egui::Key> {
    // evdev keycodes (not X11 — no +8 offset)
    match key {
        1 => Some(egui::Key::Escape),
        14 => Some(egui::Key::Backspace),
        15 => Some(egui::Key::Tab),
        28 => Some(egui::Key::Enter),
        57 => Some(egui::Key::Space),
        103 => Some(egui::Key::ArrowUp),
        108 => Some(egui::Key::ArrowDown),
        105 => Some(egui::Key::ArrowLeft),
        106 => Some(egui::Key::ArrowRight),
        // Letter keys (a=30 .. z)
        30 => Some(egui::Key::A),
        48 => Some(egui::Key::B),
        46 => Some(egui::Key::C),
        32 => Some(egui::Key::D),
        18 => Some(egui::Key::E),
        33 => Some(egui::Key::F),
        34 => Some(egui::Key::G),
        35 => Some(egui::Key::H),
        23 => Some(egui::Key::I),
        36 => Some(egui::Key::J),
        37 => Some(egui::Key::K),
        38 => Some(egui::Key::L),
        50 => Some(egui::Key::M),
        49 => Some(egui::Key::N),
        24 => Some(egui::Key::O),
        25 => Some(egui::Key::P),
        16 => Some(egui::Key::Q),
        19 => Some(egui::Key::R),
        31 => Some(egui::Key::S),
        20 => Some(egui::Key::T),
        22 => Some(egui::Key::U),
        47 => Some(egui::Key::V),
        17 => Some(egui::Key::W),
        45 => Some(egui::Key::X),
        21 => Some(egui::Key::Y),
        44 => Some(egui::Key::Z),
        // Number keys
        2 => Some(egui::Key::Num1),
        3 => Some(egui::Key::Num2),
        4 => Some(egui::Key::Num3),
        5 => Some(egui::Key::Num4),
        6 => Some(egui::Key::Num5),
        7 => Some(egui::Key::Num6),
        8 => Some(egui::Key::Num7),
        9 => Some(egui::Key::Num8),
        10 => Some(egui::Key::Num9),
        11 => Some(egui::Key::Num0),
        // Punctuation we need
        12 => Some(egui::Key::Minus),
        52 => Some(egui::Key::Period),
        53 => Some(egui::Key::Slash),
        _ => None,
    }
}

/// Map evdev keycode to printable character (for text input).
fn evdev_key_to_char(key: u32, shift: bool) -> Option<char> {
    let base = match key {
        // Letters
        30 => 'a', 48 => 'b', 46 => 'c', 32 => 'd', 18 => 'e',
        33 => 'f', 34 => 'g', 35 => 'h', 23 => 'i', 36 => 'j',
        37 => 'k', 38 => 'l', 50 => 'm', 49 => 'n', 24 => 'o',
        25 => 'p', 16 => 'q', 19 => 'r', 31 => 's', 20 => 't',
        22 => 'u', 47 => 'v', 17 => 'w', 45 => 'x', 21 => 'y',
        44 => 'z',
        // Numbers
        2 => '1', 3 => '2', 4 => '3', 5 => '4', 6 => '5',
        7 => '6', 8 => '7', 9 => '8', 10 => '9', 11 => '0',
        // Punctuation
        12 => '-', 52 => '.', 53 => '/', 57 => ' ',
        _ => return None,
    };

    if shift && base.is_ascii_lowercase() {
        Some(base.to_ascii_uppercase())
    } else {
        Some(base)
    }
}
