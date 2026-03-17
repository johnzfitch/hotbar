// Flame particle system shader
// Instanced quads -- each instance is one particle from a storage buffer

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

    // Quad corners: 0=TL, 1=TR, 2=BL, 3=BR (triangle strip)
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

// Flame color: white-yellow core -> orange -> red -> transparent
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
