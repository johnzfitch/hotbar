// Combined chrome background + heat glow shader
// Single pass with LoadOp::Clear — replaces two separate passes.
// Chrome: anisotropic noise brushed metal background
// Heat glow: fire automaton (left edge) + smoothstep glow (other edges)

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
@group(1) @binding(0) var<storage, read> fire_column: array<f32>;

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

// ── Chrome helpers ──────────────────────────────────────────────────

fn hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453123);
}

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

fn compute_chrome(uv: vec2<f32>) -> vec4<f32> {
    let base = vec3<f32>(0.051, 0.051, 0.078);

    let brush_coord = vec2<f32>(uv.x * 80.0, uv.y * 8.0 + u.time * 0.02);
    let brush = noise(brush_coord) * 0.03;

    let fine_coord = vec2<f32>(uv.x * 200.0, uv.y * 20.0);
    let fine = noise(fine_coord) * 0.015;

    let highlight = brush + fine;
    let metal = base + vec3<f32>(highlight, highlight, highlight * 1.2);

    let center = vec2<f32>(0.5, 0.5);
    let vignette = 1.0 - length(uv - center) * 0.3;

    var final_color = metal * vignette;

    if (u.scanline_lambda > 0.0) {
        let pixel_y = uv.y * u.resolution.y;
        let scanline = 0.92 + 0.08 * sin(pixel_y / u.scanline_lambda * 6.2832 + u.time * u.scanline_omega);
        final_color *= scanline;
    }

    return vec4<f32>(final_color, 0.95);
}

// ── Heat glow helpers ───────────────────────────────────────────────

fn fire_palette(heat: f32, time: f32) -> vec4<f32> {
    let h = fract(heat + time * 0.08);

    if h < 0.15 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    if h < 0.30 {
        let t = (h - 0.15) / 0.15;
        return mix(vec4<f32>(0.10, 0.0, 0.0, 0.4), vec4<f32>(0.35, 0.05, 0.0, 0.7), t);
    }
    if h < 0.50 {
        let t = (h - 0.30) / 0.20;
        return mix(vec4<f32>(0.35, 0.05, 0.0, 0.7), vec4<f32>(1.0, 0.27, 0.0, 0.9), t);
    }
    if h < 0.70 {
        let t = (h - 0.50) / 0.20;
        return mix(vec4<f32>(1.0, 0.27, 0.0, 0.9), vec4<f32>(1.0, 0.67, 0.0, 1.0), t);
    }
    if h < 0.88 {
        let t = (h - 0.70) / 0.18;
        return mix(vec4<f32>(1.0, 0.67, 0.0, 1.0), vec4<f32>(1.0, 0.93, 0.8, 1.0), t);
    }
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}

fn edge_wobble(y: f32, time: f32, intensity: f32) -> f32 {
    let n = sin(y * 0.03 + time * 1.2) * 0.6
          + sin(y * 0.07 + time * 0.8) * 0.4;
    let stepped = floor(n * 3.0) * 2.0;
    return stepped * intensity;
}

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

fn compute_heat_glow(uv: vec2<f32>) -> vec4<f32> {
    if u.heat_intensity < 0.01 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    let px = uv * u.resolution;

    // Left edge: fire automaton with Ferrari palette
    let fire_extent = 20.0 + 30.0 * u.heat_intensity;
    let wobble = edge_wobble(px.y, u.time, u.heat_intensity);
    let dist_left = px.x;
    let effective_fire_width = fire_extent + wobble;

    if dist_left < effective_fire_width {
        let y_idx = clamp(u32(px.y), 0u, u32(u.fire_height) - 1u);
        let fire_val = fire_column[y_idx];

        let effective_dist = max(dist_left - wobble, 0.0);
        let falloff = 1.0 - smoothstep(0.0, fire_extent, effective_dist);
        let sampled_heat = fire_val * falloff;

        if sampled_heat > 0.01 {
            let color = fire_palette(sampled_heat, u.time);
            if color.a > 0.001 {
                return color;
            }
        }
    }

    // Right, top, bottom edges: smoothstep glow
    let dist_right = u.resolution.x - px.x;
    let dist_top = px.y;
    let dist_bottom = u.resolution.y - px.y;
    let edge_dist = min(dist_right, min(dist_top, dist_bottom));

    let glow_width = mix(15.0, 60.0, u.heat_intensity);
    let glow = 1.0 - smoothstep(0.0, glow_width, edge_dist);
    if glow < 0.001 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    let pulse = 1.0 + sin(u.time * 4.0) * 0.15 * u.heat_intensity;
    let color = heat_color(u.heat_intensity);
    let alpha = glow * u.heat_intensity * pulse * 0.8;

    return vec4<f32>(color * alpha, alpha);
}

// ── Combined fragment shader ────────────────────────────────────────

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let chrome = compute_chrome(in.uv);
    let glow = compute_heat_glow(in.uv);

    // Additive composite: chrome base + heat glow
    return vec4<f32>(chrome.rgb + glow.rgb * glow.a, chrome.a);
}
