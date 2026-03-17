# 🔥 HOTBAR GPU Effects Specialist Prompt

## Your Role

You are building the 4 custom wgpu render passes that give hotbar its Hot Wheels Stunt Track Driver (1998) identity. These are the visual effects that transform a file browser panel into something that feels like it's ON FIRE.

You're working inside an existing codebase where egui widgets, SCTK layer-shell integration, and the daemon are already built and passing 179 tests. Your job is to add GPU-accelerated visual effects that composite with the egui layer.

## What You're Building

| Module | Shader | Purpose | Blend | Order |
|--------|--------|---------|-------|-------|
| `gpu/chrome.rs` | `shaders/chrome.wgsl` | Brushed metal background | Replaces clear color | 1st (bottom) |
| `gpu/heat_glow.rs` | `shaders/heat_glow.wgsl` | Edge glow driven by activity | Additive | 2nd |
| `gpu/flames.rs` | `shaders/flames.wgsl` | Flame particles along edges | Additive | 3rd |
| `gpu/starburst.rs` | `shaders/starburst.wgsl` | Selection explosion effect | Additive | 5th (top) |

egui renders as pass 4 (between flames and starburst).

## Existing Codebase Context

### Surface format
`wgpu::TextureFormat` — whatever `surface.get_capabilities(&adapter).formats[0]` returns. On Hyprland/Vulkan this is typically `Bgra8UnormSrgb`. The alpha mode is `PreMultiplied` for layer-shell transparency.

### Panel dimensions
- Width: 420px (`theme::PANEL_WIDTH`)
- Height: fills the screen edge (variable, set by compositor on configure)
- Available as `self.width` and `self.height` in `HotbarShell`

### SharedGpu struct (`gpu.rs`)
```rust
pub struct SharedGpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}
```

### Current render_frame() (sctk_shell.rs:336-436)

The current render pipeline is:
1. Run egui frame
2. Tessellate
3. Acquire swapchain texture → `output_frame`
4. Create view from texture
5. Create command encoder
6. Update egui textures + buffers
7. Begin render pass with `LoadOp::Clear(BG_PANEL)` 
8. Render egui into that pass via `forget_lifetime()`
9. Drop pass, submit encoder, present

**You will modify this to:**
1. Run egui frame + tessellate (unchanged)
2. Acquire swapchain texture
3. Create encoder
4. **Pass 1: Chrome background** — `LoadOp::Clear` with black, then draw fullscreen quad with metal shader
5. **Pass 2: Heat glow** — `LoadOp::Load`, additive blend, edge gradient
6. **Pass 3: Flame particles** — `LoadOp::Load`, additive blend, particle quads
7. **Pass 4: egui** — `LoadOp::Load` (NOT Clear!), standard alpha blend
8. **Pass 5: Starburst** — `LoadOp::Load`, additive blend, selection effect
9. Submit + present

### Theme colors (theme.rs)
```rust
pub const FLAME_RED: Color32 = Color32::from_rgb(0xE6, 0x1E, 0x25);
pub const FLAME_ORANGE: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x00);
pub const FLAME_YELLOW: Color32 = Color32::from_rgb(0xFF, 0xC1, 0x07);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x0D, 0x0D, 0x14);
pub const BG_SURFACE: Color32 = Color32::from_rgb(0x16, 0x16, 0x1E);
pub const CHROME: Color32 = Color32::from_rgb(0xC0, 0xC0, 0xCC);
pub const CHROME_DARK: Color32 = Color32::from_rgb(0x60, 0x60, 0x6E);
```

`theme::heat_color(intensity: f32) -> Color32` maps 0.0→cold(chrome_dark) through orange/red to 1.0→yellow.

### Spinner geometry
The spinner is a vertical carousel. The selected item sits at `center_y` of the spinner rect. Each slot is `SLOT_HEIGHT = 52.0` px. The selection highlight is a `FLAME_RED` stroke rect centered at `(rect.center_x, center_y)` with size `(rect.width - 4, SLOT_HEIGHT + 4)`.

For the starburst, you need the screen-space position of the selected slot — approximately `(panel_width / 2, panel_height * 0.5)` since the spinner occupies most of the panel.

### Data available each frame
- `activity_level: f32` — events per second (0.0 to ~20.0, normalize to 0.0..1.0 for shaders)
- `panel_width: u32`, `panel_height: u32`
- `time: f32` — elapsed seconds since startup (for animation)
- `selected_y: f32` — Y position of selected spinner slot (for starburst)
- `starburst_trigger: f32` — 1.0 on selection change, decays to 0.0 over 0.3s

---

## Architecture: gpu/mod.rs

Create `src/gpu/mod.rs` as the orchestrator for all effects:

```rust
//! GPU visual effects — flames, chrome, heat glow, starburst.
//!
//! Each effect is a separate render pass that composites with the egui layer.
//! All share the same wgpu::Device from SharedGpu.

pub mod chrome;
pub mod flames;
pub mod heat_glow;
pub mod starburst;

use crate::gpu::SharedGpu;

/// All GPU effects, initialized once and updated each frame.
pub struct GpuEffects {
    pub chrome: chrome::ChromePass,
    pub heat_glow: heat_glow::HeatGlowPass,
    pub flames: flames::FlamePass,
    pub starburst: starburst::StarburstPass,
    /// Elapsed time since startup (seconds)
    time: f32,
    /// Starburst trigger — set to 1.0 on selection change, decays each frame
    starburst_intensity: f32,
    /// Previous selected index (to detect changes)
    prev_selected: usize,
}

/// Per-frame parameters for all GPU effects.
pub struct FrameParams {
    /// Activity level: events per second, clamped to 0.0..1.0
    pub heat_intensity: f32,
    /// Panel dimensions
    pub width: u32,
    pub height: u32,
    /// Delta time in seconds
    pub dt: f32,
    /// Currently selected spinner index
    pub selected_index: usize,
    /// Y position of selected slot center (screen-space pixels)
    pub selected_y: f32,
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
    pub fn render_before_egui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        queue: &wgpu::Queue,
        params: &FrameParams,
    ) {
        self.time += params.dt;

        // Detect selection change → trigger starburst
        if params.selected_index != self.prev_selected {
            self.starburst_intensity = 1.0;
            self.prev_selected = params.selected_index;
        }
        self.starburst_intensity = (self.starburst_intensity - params.dt / 0.3).max(0.0);

        // Pass 1: Chrome background
        self.chrome.render(encoder, view, queue, params.width, params.height, self.time);

        // Pass 2: Heat glow border
        self.heat_glow.render(encoder, view, queue, params.width, params.height, params.heat_intensity, self.time);

        // Pass 3: Flame particles
        self.flames.update(queue, params.heat_intensity, params.dt, params.width, params.height);
        self.flames.render(encoder, view);
    }

    /// Render all post-egui passes (starburst).
    pub fn render_after_egui(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        queue: &wgpu::Queue,
        params: &FrameParams,
    ) {
        // Pass 5: Starburst
        if self.starburst_intensity > 0.01 {
            self.starburst.render(
                encoder, view, queue,
                params.width, params.height,
                params.selected_y / params.height as f32,
                self.starburst_intensity,
                self.time,
            );
        }
    }

    /// Resize all effect buffers.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.flames.resize(device, width, height);
    }
}
```

---

## Effect 1: Chrome Background (`gpu/chrome.rs` + `shaders/chrome.wgsl`)

Brushed metal background. Replaces the flat `BG_PANEL` clear color with a fullscreen quad that has anisotropic noise simulating directional brushing.

### Visual spec
- Base color: `BG_PANEL` (#0D0D14) with slight metallic sheen
- Vertical brush direction (noise stretched along Y axis)
- Very subtle — should look like brushed dark steel, not a mirror
- Slight vignette (darker at edges)

### WGSL shader (`shaders/chrome.wgsl`)

```wgsl
// Chrome brushed metal background shader
// Fullscreen triangle — vertex shader generates positions from vertex_index

struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle trick — 3 vertices, no vertex buffer needed
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) % 2) * 4.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// Simple hash for noise
fn hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
}

// Value noise
fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);

    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));

    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;

    // Base panel color
    let base = vec3<f32>(0.051, 0.051, 0.078); // BG_PANEL

    // Anisotropic brush noise — stretched along Y (vertical brushing)
    let brush_coord = vec2<f32>(uv.x * 80.0, uv.y * 8.0 + u.time * 0.02);
    let brush = noise(brush_coord) * 0.03;

    // Secondary finer noise layer
    let fine_coord = vec2<f32>(uv.x * 200.0, uv.y * 20.0);
    let fine = noise(fine_coord) * 0.015;

    // Combine with subtle metallic highlight
    let highlight = brush + fine;
    let metal = base + vec3<f32>(highlight, highlight, highlight * 1.2); // slight blue shift

    // Vignette — darker at edges
    let center = vec2<f32>(0.5, 0.5);
    let vignette = 1.0 - length(uv - center) * 0.3;

    let final_color = metal * vignette;

    return vec4<f32>(final_color, 0.95); // 0.95 alpha for layer-shell transparency
}
```

### Rust pipeline (`gpu/chrome.rs`)

```rust
//! Brushed chrome metal background.
//!
//! Draws a fullscreen triangle with an anisotropic noise shader
//! simulating brushed dark steel.

use wgpu::util::DeviceExt;

/// Uniform buffer for chrome shader.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ChromeUniforms {
    resolution: [f32; 2],
    time: f32,
    _pad: f32,
}

/// Chrome background render pass.
pub struct ChromePass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl ChromePass {
    /// Create the chrome pipeline.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chrome_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/chrome.wgsl").into()
            ),
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chrome_uniforms"),
            contents: bytemuck::cast_slice(&[ChromeUniforms {
                resolution: [420.0, 1080.0],
                time: 0.0,
                _pad: 0.0,
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chrome_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chrome_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chrome_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chrome_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[], // fullscreen triangle, no vertex buffer
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform_buffer,
            bind_group,
        }
    }

    /// Render the chrome background.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        time: f32,
    ) {
        // Update uniforms
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[ChromeUniforms {
                resolution: [width as f32, height as f32],
                time,
                _pad: 0.0,
            }]),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chrome_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1); // fullscreen triangle
    }
}
```

---

## Effect 2: Heat Glow (`gpu/heat_glow.rs` + `shaders/heat_glow.wgsl`)

Edge glow that intensifies with activity. At rest it's invisible. As files are written, the edges warm from subtle orange through red to flame yellow.

### Visual spec
- Glow radiates inward from all 4 panel edges
- Width: 15px at cold, expanding to 60px at max intensity
- Color: interpolated by `heat_intensity` uniform (0→transparent, 0.3→orange, 0.7→red, 1.0→yellow core)
- Additive blend — builds on top of chrome background
- Gentle pulse at high intensity (sine wave on alpha, ~2Hz)

### WGSL shader (`shaders/heat_glow.wgsl`)

```wgsl
struct Uniforms {
    resolution: vec2<f32>,
    intensity: f32,
    time: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) % 2) * 4.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

// Heat color ramp: 0→transparent, 0.3→orange, 0.7→red, 1.0→yellow
fn heat_color(t: f32) -> vec3<f32> {
    let orange = vec3<f32>(1.0, 0.42, 0.0);
    let red = vec3<f32>(0.9, 0.12, 0.15);
    let yellow = vec3<f32>(1.0, 0.76, 0.03);

    if t < 0.5 {
        return mix(orange, red, t / 0.5);
    } else {
        return mix(red, yellow, (t - 0.5) / 0.5);
    }
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if u.intensity < 0.01 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    let uv = in.uv;
    let px = uv * u.resolution;

    // Distance from nearest edge (in pixels)
    let dist_left = px.x;
    let dist_right = u.resolution.x - px.x;
    let dist_top = px.y;
    let dist_bottom = u.resolution.y - px.y;
    let edge_dist = min(min(dist_left, dist_right), min(dist_top, dist_bottom));

    // Glow width scales with intensity: 15px at low, 60px at max
    let glow_width = mix(15.0, 60.0, u.intensity);

    // Glow falloff
    let glow = 1.0 - smoothstep(0.0, glow_width, edge_dist);
    if glow < 0.001 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Pulse at high intensity
    let pulse = 1.0 + sin(u.time * 4.0) * 0.15 * u.intensity;

    // Color based on intensity
    let color = heat_color(u.intensity);

    // Final alpha: glow shape × intensity × pulse
    let alpha = glow * u.intensity * pulse * 0.8;

    // Premultiplied alpha output (for additive blending)
    return vec4<f32>(color * alpha, alpha);
}
```

### Rust pipeline (`gpu/heat_glow.rs`)

Same pattern as chrome.rs but with:
- **Additive blend state** instead of premultiplied alpha:
```rust
blend: Some(wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
}),
```
- `LoadOp::Load` (not Clear — preserves chrome background)
- Uniforms: `resolution, intensity, time`
- `render()` takes `heat_intensity: f32` parameter

---

## Effect 3: Flame Particles (`gpu/flames.rs` + `shaders/flames.wgsl`)

Particle system that spawns flames along panel edges. Intensity scales with activity level.

### Visual spec
- Particles spawn along left and right edges, rise upward
- Each particle: small soft quad (4-8px), colored yellow→orange→red→fade based on lifetime
- At cold (0 intensity): no particles
- At warm: ~20 particles, sparse, slow
- At hot: ~100 particles, dense, fast
- At on-fire: ~300 particles, roaring
- Particles have slight horizontal drift (noise-based)

### Approach
CPU-side particle update (no compute shader needed for <300 particles). Store particle data in a vertex buffer, update positions each frame on CPU, upload to GPU, draw as instanced quads.

### Particle struct
```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Particle {
    pos: [f32; 2],      // position in pixels
    vel: [f32; 2],      // velocity in pixels/sec
    life: f32,           // 1.0 → 0.0
    max_life: f32,       // initial lifetime (for color mapping)
    size: f32,           // radius in pixels
    _pad: f32,
}
```

### WGSL shader (`shaders/flames.wgsl`)

```wgsl
struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
};

struct Particle {
    pos: vec2<f32>,
    vel: vec2<f32>,
    life: f32,
    max_life: f32,
    size: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) life_ratio: f32,
    @location(2) alpha: f32,
};

// 4 vertices per particle (quad), instance_index = particle index
@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let p = particles[instance_index];

    // Quad corners: 0=TL, 1=TR, 2=BL, 3=BR
    let corner_x = f32(vertex_index & 1u) * 2.0 - 1.0;
    let corner_y = f32((vertex_index >> 1u) & 1u) * 2.0 - 1.0;

    let world_pos = p.pos + vec2<f32>(corner_x, corner_y) * p.size;

    // Convert pixel coords to NDC
    let ndc = vec2<f32>(
        (world_pos.x / u.resolution.x) * 2.0 - 1.0,
        1.0 - (world_pos.y / u.resolution.y) * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = vec2<f32>(corner_x * 0.5 + 0.5, corner_y * 0.5 + 0.5);
    out.life_ratio = p.life / p.max_life;
    out.alpha = p.life; // fade out as life decreases
    return out;
}

// Flame color: white-yellow core → orange → red → transparent
fn flame_color(life_ratio: f32) -> vec3<f32> {
    let yellow = vec3<f32>(1.0, 0.95, 0.6);
    let orange = vec3<f32>(1.0, 0.45, 0.0);
    let red = vec3<f32>(0.8, 0.1, 0.0);

    if life_ratio > 0.6 {
        return mix(orange, yellow, (life_ratio - 0.6) / 0.4);
    } else if life_ratio > 0.2 {
        return mix(red, orange, (life_ratio - 0.2) / 0.4);
    } else {
        return red * (life_ratio / 0.2);
    }
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Soft circle falloff
    let dist = length(in.uv - vec2<f32>(0.5, 0.5)) * 2.0;
    let softness = 1.0 - smoothstep(0.0, 1.0, dist);

    let color = flame_color(in.life_ratio);
    let alpha = softness * in.alpha * 0.7;

    return vec4<f32>(color * alpha, alpha);
}
```

### Rust pipeline (`gpu/flames.rs`)

Key differences from chrome/heat_glow:
- Uses a **storage buffer** for particle data (not just uniforms)
- Vertex shader uses `instance_index` to read per-particle data
- Draws `4 * active_particle_count` vertices as triangle strips (or 6 vertices with index buffer for quads)
- CPU updates particles each frame:

```rust
pub fn update(
    &mut self,
    queue: &wgpu::Queue,
    heat_intensity: f32,
    dt: f32,
    width: u32,
    height: u32,
) {
    // Spawn new particles based on intensity
    let spawn_rate = (heat_intensity * 200.0) as usize; // 0-200 per second
    let spawn_count = ((spawn_rate as f32 * dt) as usize).min(10); // cap per frame

    for _ in 0..spawn_count {
        if self.active_count >= self.particles.len() { break; }
        let side = if self.rng_state % 2 == 0 { 0.0 } else { width as f32 };
        self.particles[self.active_count] = Particle {
            pos: [side + random_spread(), random_y(height)],
            vel: [random_drift(), -30.0 - random_speed()], // rise upward
            life: 0.8 + random_life(),
            max_life: 0.8 + random_life(),
            size: 3.0 + random_size(),
            _pad: 0.0,
        };
        self.active_count += 1;
    }

    // Update existing particles
    for i in 0..self.active_count {
        self.particles[i].pos[0] += self.particles[i].vel[0] * dt;
        self.particles[i].pos[1] += self.particles[i].vel[1] * dt;
        self.particles[i].life -= dt;
    }

    // Remove dead particles (swap-remove)
    let mut i = 0;
    while i < self.active_count {
        if self.particles[i].life <= 0.0 {
            self.active_count -= 1;
            self.particles.swap(i, self.active_count);
        } else {
            i += 1;
        }
    }

    // Upload to GPU
    queue.write_buffer(
        &self.particle_buffer,
        0,
        bytemuck::cast_slice(&self.particles[..self.active_count]),
    );
}
```

Use **additive blending** and `LoadOp::Load`. Primitive topology: `TriangleStrip` with 4 vertices per instance.

---

## Effect 4: Starburst (`gpu/starburst.rs` + `shaders/starburst.wgsl`)

Selection change explosion. When the user selects a new file in the spinner, rays emanate from the selection point and decay over 0.3 seconds.

### Visual spec
- 12-16 rays emanating from selection center point
- Rays start short + bright, extend outward as they fade
- Color: flame yellow core → orange edges → transparent
- Duration: 0.3s (triggered by selection change, driven by `starburst_intensity` uniform decaying from 1.0 to 0.0)
- Additive blend — glows on top of everything

### WGSL shader (`shaders/starburst.wgsl`)

```wgsl
struct Uniforms {
    resolution: vec2<f32>,
    center_y: f32,       // normalized Y position of selection (0.0..1.0)
    intensity: f32,      // 1.0 → 0.0 over 0.3s
    time: f32,
    _pad: vec3<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) % 2) * 4.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 1.0 - (y * 0.5 + 0.5));
    return out;
}

const PI: f32 = 3.14159265359;
const NUM_RAYS: f32 = 14.0;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if u.intensity < 0.01 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Center of burst in UV space (horizontally centered, at selection Y)
    let center = vec2<f32>(0.5, u.center_y);
    let delta = in.uv - center;

    // Aspect ratio correction
    let aspect = u.resolution.x / u.resolution.y;
    let corrected = vec2<f32>(delta.x * aspect, delta.y);

    let dist = length(corrected);
    let angle = atan2(corrected.y, corrected.x);

    // Ray pattern — cosine-based spikes
    let ray = pow(abs(cos(angle * NUM_RAYS / 2.0)), 8.0);

    // Radial falloff — rays extend outward as intensity increases
    let max_radius = 0.15 + (1.0 - u.intensity) * 0.2; // rays grow as burst decays
    let radial = 1.0 - smoothstep(0.0, max_radius, dist);

    // Core glow (always present during burst)
    let core = exp(-dist * 30.0) * u.intensity;

    // Combine rays + core
    let brightness = (ray * radial * 0.6 + core) * u.intensity;

    // Color: yellow core, orange rays
    let yellow = vec3<f32>(1.0, 0.9, 0.3);
    let orange = vec3<f32>(1.0, 0.5, 0.0);
    let color = mix(orange, yellow, core / (brightness + 0.001));

    let alpha = brightness * 0.9;

    return vec4<f32>(color * alpha, alpha);
}
```

Same pipeline pattern as heat_glow (fullscreen triangle, additive blend, `LoadOp::Load`). Uniforms include `center_y` and `intensity`.

---

## Modified render_frame()

Here's the updated `render_frame()` method in `sctk_shell.rs`. The key changes are marked with `// NEW`:

```rust
fn render_frame(&mut self) {
    let Some(gpu) = &self.gpu else { return };
    let Some(surface) = &self.wgpu_surface else { return };
    let Some(renderer) = &mut self.egui_renderer else { return };
    let Some(surface_config) = &self.surface_config else { return };
    let Some(effects) = &mut self.gpu_effects else { return }; // NEW

    // Prepare egui input
    let mut input = self.egui_input.take();
    input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(self.width as f32, self.height as f32),
    ));

    // Apply theme
    theme::apply_theme(&self.egui_ctx);

    // Run egui frame
    let full_output = self.egui_ctx.run(input, |ctx| {
        if let Some(cb) = &mut self.ui_callback {
            cb(ctx);
        }
    });

    // Tessellate
    let clipped_primitives = self.egui_ctx.tessellate(
        full_output.shapes,
        full_output.pixels_per_point,
    );

    // Acquire frame
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

    let view = output_frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Update egui textures
    for (id, delta) in &full_output.textures_delta.set {
        renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
    }

    let screen_descriptor = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [self.width, self.height],
        pixels_per_point: full_output.pixels_per_point,
    };

    let mut encoder = gpu.device.create_command_encoder(
        &wgpu::CommandEncoderDescriptor { label: Some("hotbar_encoder") },
    );

    // NEW: Prepare frame params
    let dt = 1.0 / 60.0; // TODO: measure actual frame time
    let params = crate::gpu::FrameParams {
        heat_intensity: self.heat_intensity.clamp(0.0, 1.0), // set by daemon
        width: self.width,
        height: self.height,
        dt,
        selected_index: self.selected_index, // set by app
        selected_y: self.height as f32 * 0.5, // approximate center
    };

    // NEW: Passes 1-3 (chrome, heat glow, flames) — BEFORE egui
    effects.render_before_egui(&mut encoder, &view, &gpu.queue, &params);

    // Pass 4: egui — note LoadOp::Load (not Clear!)
    renderer.update_buffers(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &clipped_primitives,
        &screen_descriptor,
    );

    let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("egui_render_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load, // CHANGED: was Clear, now Load
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    let mut render_pass = render_pass.forget_lifetime();
    renderer.render(&mut render_pass, &clipped_primitives, &screen_descriptor);
    drop(render_pass);

    // NEW: Pass 5 (starburst) — AFTER egui
    effects.render_after_egui(&mut encoder, &view, &gpu.queue, &params);

    gpu.queue.submit(std::iter::once(encoder.finish()));
    output_frame.present();

    // Free textures
    for id in &full_output.textures_delta.free {
        renderer.free_texture(id);
    }
}
```

## New fields on HotbarShell

Add to the struct:
```rust
/// GPU visual effects (flames, chrome, heat glow, starburst)
gpu_effects: Option<GpuEffects>,
/// Current heat intensity (set by daemon via Arc<RwLock<HotState>>)
heat_intensity: f32,
/// Current selected spinner index (set by app each frame)
selected_index: usize,
```

Initialize `gpu_effects` in `init_gpu()` after creating the egui renderer:
```rust
let effects = GpuEffects::new(&gpu.device, format, self.width, self.height);
self.gpu_effects = Some(effects);
```

## Dependencies to add to Cargo.toml

```toml
bytemuck = { version = "1", features = ["derive"] }
```

This is already in the plan's dependency budget.

---

## Testing

GPU effects are visual — there's no unit test for "do the flames look right." But you CAN test:

1. **Pipeline creation** — `ChromePass::new()` doesn't panic with a valid device
2. **Uniform upload** — `chrome.render()` with a mock encoder doesn't panic
3. **Particle physics** — `FlamePass::update()` correctly spawns, ages, and removes particles
4. **Starburst trigger** — intensity decays from 1.0 to 0.0 over ~0.3s

For pipeline creation tests, use `wgpu::Backends::GL` with `WGPU_BACKEND=gl` and mark as `#[ignore]` for CI.

## Checklist

```
□ gpu/mod.rs — GpuEffects orchestrator with render_before_egui / render_after_egui
□ gpu/chrome.rs + shaders/chrome.wgsl — brushed metal background
□ gpu/heat_glow.rs + shaders/heat_glow.wgsl — edge glow driven by activity
□ gpu/flames.rs + shaders/flames.wgsl — particle system along edges
□ gpu/starburst.rs + shaders/starburst.wgsl — selection change explosion
□ sctk_shell.rs — modified render_frame() with 5-pass pipeline
□ sctk_shell.rs — GpuEffects initialized in init_gpu()
□ sctk_shell.rs — heat_intensity and selected_index wired from daemon/app state
□ cargo check passes
□ cargo clippy clean
□ Particle tests pass
□ All shaders compile (validated by pipeline creation)
```
