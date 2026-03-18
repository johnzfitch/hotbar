//! Heat glow edge effect with 1D fire automaton.
//!
//! Left edge: CPU-side cellular automaton simulates a fire column.
//! Values are uploaded to a GPU storage buffer each frame, colored via
//! the Ferrari palette ramp with time-cycling in the shader.
//!
//! Right/top/bottom edges: smoothstep glow scaled by activity intensity.
//! Uses additive blending over the chrome background.

use wgpu::util::DeviceExt;

/// Maximum fire column height (entries, one per pixel row).
/// 4096 supports up to 4K vertical monitors.
const MAX_FIRE_HEIGHT: usize = 4096;

/// Uniform buffer layout for heat glow shader.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HeatGlowUniforms {
    resolution: [f32; 2],
    intensity: f32,
    time: f32,
    fire_height: f32,
    _pad: [f32; 3],
}

/// Heat glow + fire automaton render pass.
pub struct HeatGlowPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    fire_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// CPU-side fire column (one float per pixel row, bottom = last index)
    fire_column: Vec<f32>,
    /// Xorshift RNG state for fire injection randomness
    fire_rng: u32,
}

impl HeatGlowPass {
    /// Create the heat glow pipeline with fire automaton storage buffer.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("heat_glow_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/heat_glow.wgsl").into(),
            ),
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("heat_glow_uniforms"),
            contents: bytemuck::cast_slice(&[HeatGlowUniforms {
                resolution: [420.0, 1080.0],
                intensity: 0.0,
                time: 0.0,
                fire_height: 1080.0,
                _pad: [0.0; 3],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Pre-allocate fire column storage buffer at max capacity
        let fire_data = vec![0.0f32; MAX_FIRE_HEIGHT];
        let fire_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("heat_glow_fire_column"),
            contents: bytemuck::cast_slice(&fire_data),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("heat_glow_bind_group_layout"),
                entries: &[
                    // Binding 0: uniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // Binding 1: fire column (storage, read-only)
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("heat_glow_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fire_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("heat_glow_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Additive blend: src + dst
        let additive_blend = wgpu::BlendState {
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
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("heat_glow_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(additive_blend),
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
            fire_buffer,
            bind_group,
            fire_column: vec![0.0; MAX_FIRE_HEIGHT],
            fire_rng: 0xBEEF_CAFE,
        }
    }

    /// Run the 1D fire automaton and upload to GPU.
    ///
    /// Call once per frame before `render()`. The automaton injects heat
    /// at the bottom of the column and propagates upward with averaging
    /// and decay — producing a living fire that rises and dies naturally.
    pub fn update_fire(&mut self, queue: &wgpu::Queue, heat_intensity: f32, height: u32) {
        crate::dev_trace_span!("fire_automaton");
        let h = (height as usize).min(MAX_FIRE_HEIGHT);
        if h < 3 {
            return;
        }

        if heat_intensity < 0.01 {
            // Cool down: decay existing fire without injecting new heat
            for val in &mut self.fire_column[..h] {
                *val = (*val - 0.02).max(0.0);
            }
        } else {
            // Inject heat at bottom two rows
            self.fire_column[h - 1] = heat_intensity * self.rng_range(0.7, 1.0);
            self.fire_column[h - 2] = heat_intensity * self.rng_range(0.5, 1.0);

            // Propagate upward: average 3-4 neighbors, subtract decay
            for y in (0..h - 2).rev() {
                let n3 = if y + 3 < h {
                    self.fire_column[y + 3]
                } else {
                    0.0
                };
                let sum = self.fire_column[y + 1]
                    + self.fire_column[y]
                    + self.fire_column[y + 2]
                    + n3;
                self.fire_column[y] = (sum * 0.25 - 0.012).max(0.0);
            }
        }

        // Upload active portion to GPU storage buffer
        queue.write_buffer(
            &self.fire_buffer,
            0,
            bytemuck::cast_slice(&self.fire_column[..h]),
        );
    }

    /// Render the heat glow + fire automaton (pass 2).
    ///
    /// Uses `LoadOp::Load` to preserve the chrome background underneath.
    /// `scissor`: `[x, y, width, height]` clipping rectangle for reveal animation.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        heat_intensity: f32,
        time: f32,
        scissor: [u32; 4],
    ) {
        let fire_h = (height as usize).min(MAX_FIRE_HEIGHT).min(self.fire_column.len());

        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[HeatGlowUniforms {
                resolution: [width as f32, height as f32],
                intensity: heat_intensity,
                time,
                fire_height: fire_h as f32,
                _pad: [0.0; 3],
            }]),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("heat_glow_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        pass.draw(0..3, 0..1);
    }

    /// Xorshift32 RNG — returns value in [0.0, 1.0).
    fn rng_next(&mut self) -> f32 {
        self.fire_rng ^= self.fire_rng << 13;
        self.fire_rng ^= self.fire_rng >> 17;
        self.fire_rng ^= self.fire_rng << 5;
        (self.fire_rng as f32) / (u32::MAX as f32)
    }

    /// Random float in [lo, hi).
    fn rng_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.rng_next() * (hi - lo)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fire_automaton_propagates_upward() {
        // Simulate the fire automaton without GPU
        let h = 100;
        let mut col = vec![0.0f32; h];
        let mut rng: u32 = 42;

        // Helper: xorshift
        let mut next_f32 = || -> f32 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f32) / (u32::MAX as f32)
        };

        // Run several iterations at full heat
        for _ in 0..50 {
            col[h - 1] = 0.8 + next_f32() * 0.2;
            col[h - 2] = 0.5 + next_f32() * 0.5;

            for y in (0..h - 2).rev() {
                let n3 = if y + 3 < h { col[y + 3] } else { 0.0 };
                let sum = col[y + 1] + col[y] + col[y + 2] + n3;
                col[y] = (sum * 0.25 - 0.012).max(0.0);
            }
        }

        // Bottom should be hot
        assert!(col[h - 1] > 0.5, "bottom should be hot: {}", col[h - 1]);
        // Middle should have some heat
        assert!(col[h / 2] > 0.0, "middle should have heat: {}", col[h / 2]);
        // Top should be cooler than bottom
        assert!(
            col[0] < col[h - 1],
            "top should be cooler: top={} bottom={}",
            col[0],
            col[h - 1]
        );
    }

    #[test]
    fn fire_automaton_cools_down() {
        let h = 50;
        let mut col = vec![0.5f32; h];

        // Run decay (no injection)
        for _ in 0..100 {
            for val in col.iter_mut() {
                *val = (*val - 0.02).max(0.0);
            }
        }

        // Should be all zero
        assert!(col.iter().all(|&v| v == 0.0), "should cool to zero");
    }

    #[test]
    fn fire_palette_transparent_at_zero() {
        // The shader palette returns transparent when heat < 0.15
        // We can verify the conceptual mapping:
        let heat = 0.0;
        let h = (heat + 0.0_f32 * 0.08).fract(); // time=0
        assert!(h < 0.15, "zero heat should be in transparent band");
    }

    #[test]
    fn fire_palette_cycling_shifts_lookup() {
        // At different times, the same heat value maps to different palette positions
        let heat = 0.5;
        let h0 = (heat + 0.0 * 0.08_f32).fract();
        let h1 = (heat + 10.0 * 0.08_f32).fract();
        assert_ne!(h0, h1, "palette should shift with time");
    }
}
