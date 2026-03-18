//! Combined chrome background + heat glow pass.
//!
//! Merges the brushed metal background and fire automaton edge glow into
//! a single render pass with `LoadOp::Clear`. Eliminates one render pass
//! roundtrip per frame (5 passes → 4).
//!
//! Uses shared uniforms (group 0) and fire column storage (group 1).

use wgpu::util::DeviceExt;

use super::heat_glow::MAX_FIRE_HEIGHT;

/// Combined chrome + heat glow render pass.
pub struct ChromeHeatPass {
    pipeline: wgpu::RenderPipeline,
    fire_buffer: wgpu::Buffer,
    bind_group_0: wgpu::BindGroup,
    bind_group_1: wgpu::BindGroup,
    /// CPU-side fire column (one float per pixel row, bottom = last index)
    fire_column: Vec<f32>,
    /// Xorshift RNG state for fire injection randomness
    fire_rng: u32,
}

impl ChromeHeatPass {
    /// Create the combined chrome + heat glow pipeline.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shared_uniform_buffer: &wgpu::Buffer,
        shared_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chrome_heat_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/chrome_heat.wgsl").into(),
            ),
        });

        // Fire column storage buffer
        let fire_data = vec![0.0f32; MAX_FIRE_HEIGHT];
        let fire_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chrome_heat_fire_column"),
            contents: bytemuck::cast_slice(&fire_data),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Group 0: shared uniforms
        let bind_group_0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chrome_heat_bg0"),
            layout: shared_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shared_uniform_buffer.as_entire_binding(),
            }],
        });

        // Group 1: fire column storage
        let fire_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chrome_heat_fire_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group_1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chrome_heat_bg1"),
            layout: &fire_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: fire_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chrome_heat_pipeline_layout"),
            bind_group_layouts: &[shared_bgl, &fire_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chrome_heat_pipeline"),
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
            fire_buffer,
            bind_group_0,
            bind_group_1,
            fire_column: vec![0.0; MAX_FIRE_HEIGHT],
            fire_rng: 0xBEEF_CAFE,
        }
    }

    /// Run the 1D fire automaton and upload to GPU.
    ///
    /// Identical logic to the former `HeatGlowPass::update_fire`.
    pub fn update_fire(&mut self, queue: &wgpu::Queue, heat_intensity: f32, height: u32) {
        crate::dev_trace_span!("fire_automaton");
        let h = (height as usize).min(MAX_FIRE_HEIGHT);
        if h < 3 {
            return;
        }

        if heat_intensity < 0.01 {
            for val in &mut self.fire_column[..h] {
                *val = (*val - 0.02).max(0.0);
            }
        } else {
            self.fire_column[h - 1] = heat_intensity * self.rng_range(0.7, 1.0);
            self.fire_column[h - 2] = heat_intensity * self.rng_range(0.5, 1.0);

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

        queue.write_buffer(
            &self.fire_buffer,
            0,
            bytemuck::cast_slice(&self.fire_column[..h]),
        );
    }

    /// Render chrome background + heat glow in a single pass.
    ///
    /// Uses `LoadOp::Clear` — this is pass 1 (bottom layer).
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        scissor: [u32; 4],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chrome_heat_pass"),
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
        pass.set_bind_group(0, &self.bind_group_0, &[]);
        pass.set_bind_group(1, &self.bind_group_1, &[]);
        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        pass.draw(0..3, 0..1);
    }

    /// Find Y positions where fire column exceeds the given heat threshold.
    pub fn hot_spots(&self, threshold: f32, height: u32) -> Vec<f32> {
        super::heat_glow::scan_hot_spots(&self.fire_column, threshold, height)
    }

    /// Number of entries in the fire column.
    pub fn fire_column_len(&self) -> usize {
        self.fire_column.len()
    }

    /// Xorshift32 RNG.
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
