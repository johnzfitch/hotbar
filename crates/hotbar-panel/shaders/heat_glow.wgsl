// Heat glow edge effect -- intensity driven by activity level
// Additive blend over chrome background

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

// Heat color ramp: 0->orange, 0.5->red, 1.0->yellow
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

    // Pulse at high intensity (~2Hz)
    let pulse = 1.0 + sin(u.time * 4.0) * 0.15 * u.intensity;

    // Color based on intensity
    let color = heat_color(u.intensity);

    // Final alpha: glow shape x intensity x pulse
    let alpha = glow * u.intensity * pulse * 0.8;

    // Premultiplied alpha output (for additive blending)
    return vec4<f32>(color * alpha, alpha);
}
