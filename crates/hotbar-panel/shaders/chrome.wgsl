// Chrome brushed metal background shader
// Fullscreen triangle -- vertex shader generates positions from vertex_index

struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    heat_intensity: f32,
    fire_height: f32,
    scanline_lambda: f32,
    scanline_omega: f32,
    starburst_center_y: f32,
    starburst_intensity: f32,
    _pad: vec3<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle trick -- 3 vertices, no vertex buffer needed
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

    // Base panel color (BG_PANEL: #0D0D14)
    let base = vec3<f32>(0.051, 0.051, 0.078);

    // Anisotropic brush noise -- stretched along Y (vertical brushing)
    let brush_coord = vec2<f32>(uv.x * 80.0, uv.y * 8.0 + u.time * 0.02);
    let brush = noise(brush_coord) * 0.03;

    // Secondary finer noise layer
    let fine_coord = vec2<f32>(uv.x * 200.0, uv.y * 20.0);
    let fine = noise(fine_coord) * 0.015;

    // Combine with subtle metallic highlight (slight blue shift)
    let highlight = brush + fine;
    let metal = base + vec3<f32>(highlight, highlight, highlight * 1.2);

    // Vignette -- darker at edges
    let center = vec2<f32>(0.5, 0.5);
    let vignette = 1.0 - length(uv - center) * 0.3;

    var final_color = metal * vignette;

    // Scan-line overlay -- horizontal lines that scroll with time.
    // During reveal Crack phase: tight (lambda=3) and frenetic (omega=12).
    // At idle: wide (lambda=8) and barely perceptible (omega=2).
    if (u.scanline_lambda > 0.0) {
        let pixel_y = uv.y * u.resolution.y;
        let scanline = 0.92 + 0.08 * sin(pixel_y / u.scanline_lambda * 6.2832 + u.time * u.scanline_omega);
        final_color *= scanline;
    }

    return vec4<f32>(final_color, 0.95); // 0.95 alpha for layer-shell transparency
}
