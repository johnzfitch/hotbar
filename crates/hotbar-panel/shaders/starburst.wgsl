// Starburst selection explosion effect
// Fullscreen triangle, additive blend, decays over ~0.3s

struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    heat_intensity: f32,
    fire_height: f32,
    scanline_lambda: f32,
    scanline_omega: f32,
    starburst_center_y: f32,  // normalized Y position of selection (0.0..1.0)
    starburst_intensity: f32, // 1.0 -> 0.0 over 0.3s
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
    if u.starburst_intensity < 0.01 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Center of burst in UV space (horizontally centered, at selection Y)
    let center = vec2<f32>(0.5, u.starburst_center_y);
    let delta = in.uv - center;

    // Aspect ratio correction
    let aspect = u.resolution.x / u.resolution.y;
    let corrected = vec2<f32>(delta.x * aspect, delta.y);

    let dist = length(corrected);
    let angle = atan2(corrected.y, corrected.x);

    // Ray pattern -- cosine-based spikes
    let ray = pow(abs(cos(angle * NUM_RAYS / 2.0)), 8.0);

    // Radial falloff -- rays extend outward as intensity increases
    let max_radius = 0.15 + (1.0 - u.starburst_intensity) * 0.2;
    let radial = 1.0 - smoothstep(0.0, max_radius, dist);

    // Core glow (always present during burst)
    let core = exp(-dist * 30.0) * u.starburst_intensity;

    // Combine rays + core
    let brightness = (ray * radial * 0.6 + core) * u.starburst_intensity;

    // Color: yellow core, orange rays
    let yellow = vec3<f32>(1.0, 0.9, 0.3);
    let orange = vec3<f32>(1.0, 0.5, 0.0);
    let color = mix(orange, yellow, core / (brightness + 0.001));

    let alpha = brightness * 0.9;

    return vec4<f32>(color * alpha, alpha);
}
