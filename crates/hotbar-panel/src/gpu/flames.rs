//! Flame particle system along panel edges.
//!
//! CPU-side particle simulation uploaded to a GPU storage buffer each frame.
//! Particles spawn along left and right edges, rise upward with drift.
//! Intensity scales with activity level.

use wgpu::util::DeviceExt;

/// Maximum number of particles in the system.
const MAX_PARTICLES: usize = 512;

/// Per-particle data, uploaded to GPU storage buffer.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Particle {
    pos: [f32; 2],
    vel: [f32; 2],
    life: f32,
    max_life: f32,
    size: f32,
    _pad: f32,
}

impl Default for Particle {
    fn default() -> Self {
        Self {
            pos: [0.0; 2],
            vel: [0.0; 2],
            life: 0.0,
            max_life: 1.0,
            size: 0.0,
            _pad: 0.0,
        }
    }
}

/// Simple xorshift RNG for deterministic particle spawning.
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    /// Returns a value in [0.0, 1.0).
    fn next_f32(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        (self.state as f32) / (u32::MAX as f32)
    }

    /// Returns a value in [lo, hi).
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

/// Flame particle render pass.
///
/// Uses the shared uniform buffer (group 0) plus its own particle
/// storage buffer (group 1).
pub struct FlamePass {
    pipeline: wgpu::RenderPipeline,
    particle_buffer: wgpu::Buffer,
    bind_group_0: wgpu::BindGroup,
    bind_group_1: wgpu::BindGroup,
    particles: Vec<Particle>,
    active_count: usize,
    rng: Rng,
}

impl FlamePass {
    /// Create the flame pipeline.
    ///
    /// `shared_uniform_buffer`: the single uniform buffer written once per frame.
    /// `shared_bgl`: bind group layout for group 0 (shared uniforms).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        shared_uniform_buffer: &wgpu::Buffer,
        shared_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let _ = (width, height); // used for initial uniform values, now in shared buffer

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flames_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/flames.wgsl").into(),
            ),
        });

        // Pre-allocate particle storage buffer at max capacity
        let particles = vec![Particle::default(); MAX_PARTICLES];
        let particle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flames_particles"),
            contents: bytemuck::cast_slice(&particles),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Group 0: shared uniforms
        let bind_group_0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flames_bg0"),
            layout: shared_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shared_uniform_buffer.as_entire_binding(),
            }],
        });

        // Group 1: particle storage
        let particle_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flames_particle_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group_1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flames_bg1"),
            layout: &particle_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: particle_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flames_pipeline_layout"),
            bind_group_layouts: &[shared_bgl, &particle_bgl],
            push_constant_ranges: &[],
        });

        // Additive blend for flame particles
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
            label: Some("flames_pipeline"),
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
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            particle_buffer,
            bind_group_0,
            bind_group_1,
            particles,
            active_count: 0,
            rng: Rng::new(0xDEAD_BEEF),
        }
    }

    /// Update particle simulation and upload to GPU.
    ///
    /// Spawns new particles based on intensity, advances physics,
    /// removes dead particles via swap-remove.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        heat_intensity: f32,
        dt: f32,
        width: u32,
        height: u32,
    ) {
        crate::dev_trace_span!("flames_sim", particles = self.active_count);
        let width_f = width as f32;
        let height_f = height as f32;

        // Spawn new particles based on intensity (0-200 per second)
        let spawn_rate = (heat_intensity * 200.0) as usize;
        let spawn_count = ((spawn_rate as f32 * dt) as usize).min(10);

        for _ in 0..spawn_count {
            if self.active_count >= MAX_PARTICLES {
                break;
            }
            // Alternate spawning on left and right edges
            let side = if self.rng.next_f32() < 0.5 {
                self.rng.range(-2.0, 4.0)
            } else {
                width_f + self.rng.range(-4.0, 2.0)
            };

            let life = 0.8 + self.rng.range(0.0, 0.6);
            self.particles[self.active_count] = Particle {
                pos: [side, self.rng.range(height_f * 0.1, height_f * 0.95)],
                vel: [
                    self.rng.range(-8.0, 8.0),                  // horizontal drift
                    -(30.0 + self.rng.range(0.0, 40.0)),        // rise upward
                ],
                life,
                max_life: life,
                size: 3.0 + self.rng.range(0.0, 5.0),
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

        // Upload active particles to GPU
        if self.active_count > 0 {
            queue.write_buffer(
                &self.particle_buffer,
                0,
                bytemuck::cast_slice(&self.particles[..self.active_count]),
            );
        }

        // Uniforms are written via the shared uniform buffer in GpuEffects.
    }

    /// Render flame particles (pass 3).
    ///
    /// Uses `LoadOp::Load` + additive blend. Each particle is a 4-vertex
    /// triangle strip instance.
    /// `scissor`: `[x, y, width, height]` clipping rectangle for reveal animation.
    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, scissor: [u32; 4]) {
        crate::dev_trace_span!("flames_encode", particles = self.active_count);
        if self.active_count == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("flames_pass"),
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
        pass.set_bind_group(0, &self.bind_group_0, &[]);
        pass.set_bind_group(1, &self.bind_group_1, &[]);
        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
        // 4 vertices per quad (triangle strip), one instance per active particle
        pass.draw(0..4, 0..self.active_count as u32);
    }

    /// Resize: no-op for flames (particles adapt to new dimensions in update).
    pub fn resize(&mut self, _device: &wgpu::Device, _width: u32, _height: u32) {
        // Particle positions are in pixel space; they naturally adapt
        // to new dimensions on the next update() call.
    }

    /// Number of currently active particles (for diagnostics).
    pub fn active_particle_count(&self) -> usize {
        self.active_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particle_spawning() {
        // Test the particle spawn/update/remove lifecycle without GPU
        let mut particles = vec![Particle::default(); MAX_PARTICLES];
        let mut active = 0;
        let mut rng = Rng::new(42);

        // Spawn 5 particles
        for _ in 0..5 {
            particles[active] = Particle {
                pos: [0.0, rng.range(0.0, 1080.0)],
                vel: [0.0, -40.0],
                life: 1.0,
                max_life: 1.0,
                size: 4.0,
                _pad: 0.0,
            };
            active += 1;
        }
        assert_eq!(active, 5);

        // Advance time by 0.5s — all still alive
        for p in particles.iter_mut().take(active) {
            p.life -= 0.5;
        }
        assert!(particles[0].life > 0.0);

        // Advance another 0.6s — all dead
        for p in particles.iter_mut().take(active) {
            p.life -= 0.6;
        }

        // Swap-remove dead
        let mut i = 0;
        while i < active {
            if particles[i].life <= 0.0 {
                active -= 1;
                particles.swap(i, active);
            } else {
                i += 1;
            }
        }
        assert_eq!(active, 0);
    }

    #[test]
    fn rng_produces_values_in_range() {
        let mut rng = Rng::new(12345);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "value out of range: {v}");
        }
    }

    #[test]
    fn rng_range_produces_values_in_range() {
        let mut rng = Rng::new(99);
        for _ in 0..1000 {
            let v = rng.range(10.0, 20.0);
            assert!(
                (10.0..20.0).contains(&v),
                "value out of range: {v}"
            );
        }
    }
}
