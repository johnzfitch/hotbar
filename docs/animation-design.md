# Forgeheat Panel — Animation Design v2

## Architecture constraints (non-negotiable)

- **Surface**: Wayland layer-shell, anchored TOP|RIGHT|BOTTOM, 8px margin
- **Rendering**: wgpu + egui hybrid. Shader effects in `heat_glow.wgsl`, UI elements via egui `Painter`
- **Panel width**: 420px usable → request 440px surface (20px transparent bleed zone)
- **Animation control**: `layer_surface.set_margin()` for position, shader uniforms for visual effects
- **Particle system**: Already exists, supports arbitrary spawn positions via `FrameParams`
- **State tracking**: `last_write_time: Instant` per `HotFile`, agent connection events from events.jsonl/JSONL ingest

## Design philosophy

Two simultaneous animation registers:

| Register | Technique era | Used for | Character |
|---|---|---|---|
| **Discrete** | 90s adventure games | Focal elements: file indicators, torch sprites, flicker states | Crafted, legible, authorial |
| **Continuous** | 2000s game engines | Ambient effects: panel motion, glow, shake, chromatic split | Physical, energetic, mathematical |

The discrete layer anchors attention. The continuous layer provides environment.
These coexist without competing — identical to how UT2004's HUD paired sharp kill-stamps
with smooth adrenaline pulses.

---

## Shared math primitives

All animation curves derive from solutions to the damped harmonic oscillator:

```
ẍ + 2ζωₙẋ + ωₙ²x = 0
```

We use three damping regimes throughout:

```rust
/// Underdamped spring (ζ ≈ 0.4) — overshoots and rocks. For heavy mechanical elements.
fn underdamped(t: f32, target: f32, overshoot: f32, freq: f32, decay: f32) -> f32 {
    target + overshoot * (-decay * t).exp() * (freq * t).sin()
}

/// Critically damped (ζ = 1.0) — snaps into place, no oscillation. For text/data stamps.
fn critically_damped(t: f32, initial_offset: f32, rate: f32) -> f32 {
    1.0 + initial_offset * (-rate * t).exp()
}

/// Overdamped (ζ > 1) — sluggish settle. For ambient glow decay after spikes.
fn overdamped(t: f32, amplitude: f32, fast_decay: f32, slow_decay: f32) -> f32 {
    amplitude * (0.6 * (-fast_decay * t).exp() + 0.4 * (-slow_decay * t).exp())
}
```

Additional shared functions:

```rust
/// Squared sine — sharper peak, longer trough. UT2004 adrenaline pulse shape.
/// Reads as "heartbeat" rather than "breathing."
fn squared_sine(t: f32, period: f32) -> f32 {
    let s = (std::f32::consts::PI * t / period).sin();
    s * s
}

/// Concave fade — fast initial drop, long ghostly tail. Renegade segment dissolution.
/// Exponent 0.3 gives the "ghost lingers" quality.
fn concave_fade(t: f32, duration: f32) -> f32 {
    let norm = (t / duration).clamp(0.0, 1.0);
    1.0 - norm.powf(0.3)
}

/// Perlin-envelope shake — FlatOut impact model.
fn shake_offset(t: f32, amplitude: f32, decay: f32, freq: f32, seed: u64) -> Vec2 {
    let envelope = amplitude * (-decay * t).exp();
    if envelope < 0.3 { return Vec2::ZERO; }
    vec2(
        perlin_1d(t * freq, seed) * envelope,
        perlin_1d(t * freq, seed + 1000) * envelope,
    )
}
```

---

## 1. Panel entrance — "Ignition Entry"

**Sources**: Full Throttle unroll + Myst mechanical overshoot + Renegade scan-line + UT chromatic aberration + FlatOut motion blur

Three phases, total duration ~350ms:

### Phase A — Crack (0–80ms)

The panel appears at final X position but clipped to a 2px vertical slit.

| Channel | Behavior |
|---|---|
| `reveal_width` | Holds at 2.0px |
| `heat_intensity` | 1.0 (furnace brightness through the crack) |
| Scan-line mask | Active, tight λ=3px, fast scroll ω=12.0 — CRT ignition flicker |
| Particles | 2-3 embers per frame spawning along the 2px slit line |

**Implementation**: Shader uniform `u_reveal_width: f32` controls a `step()` clip in the main fragment pass. Content left of the clip boundary renders transparent.

```wgsl
// In panel composite shader
let visible = step(u_reveal_width, panel_local_x);
// Everything to the right of reveal_width is clipped
final_color = mix(vec4(0.0), content_color, visible);

// Scan-line interference (Renegade model)
let scan = 0.85 + 0.15 * sin(2.0 * PI * frag_y / u_scanline_lambda - u_time * u_scanline_omega);
final_color *= scan;
```

### Phase B — Slam open (80–220ms)

`reveal_width` lerps from 2px to 440px (20px overshoot past usable width) with cubic ease-out.

| Channel | Behavior |
|---|---|
| `reveal_width` | `2.0 + ease_out_cubic(t_local) * 438.0` |
| `heat_intensity` | 0.9 (still near-max) |
| Chromatic aberration | `d = clamp(d_reveal/dt * 0.008, 0.0, 3.0)` — RGB splits proportional to reveal velocity |
| Motion blur ghosts | 3 ghost copies at 50%/33%/25% opacity, displaced +2/+4/+6px rightward in shader |
| Scan-line λ | Widens from 3px → 6px as surface stabilizes |
| Particles | Fire pours from the leading reveal edge (spawn X = reveal_width) |

```rust
fn phase_b_reveal(t_local: f32) -> f32 {
    // t_local: 0.0 to 1.0 over 140ms
    let ease = 1.0 - (1.0 - t_local).powi(3);  // cubic ease-out
    2.0 + ease * 438.0
}

fn chromatic_displacement(reveal_width_prev: f32, reveal_width_now: f32, dt: f32) -> f32 {
    let velocity = (reveal_width_now - reveal_width_prev) / dt;
    (velocity * 0.008).clamp(0.0, 3.0)
}
```

```wgsl
// Chromatic aberration in composite pass (Renegade holographic fringe)
let d = u_chromatic_offset;
let r = textureSample(panel_tex, samp, uv + vec2(d / width, 0.0)).r;
let g = textureSample(panel_tex, samp, uv).g;
let b = textureSample(panel_tex, samp, uv - vec2(d / width, 0.0)).b;
final_color = vec4(r, g, b, final_color.a);

// Motion blur ghosts (FlatOut model — 4 copies along velocity vector)
let ghost1 = textureSample(panel_tex, samp, uv + vec2(2.0 / width, 0.0)) * 0.5;
let ghost2 = textureSample(panel_tex, samp, uv + vec2(4.0 / width, 0.0)) * 0.33;
let ghost3 = textureSample(panel_tex, samp, uv + vec2(6.0 / width, 0.0)) * 0.25;
final_color = max(final_color, max(ghost1, max(ghost2, ghost3))); // additive-ish blend
```

### Phase C — Settle (220–350ms)

Underdamped oscillation. The panel rocks past target and settles.

| Channel | Behavior |
|---|---|
| `reveal_width` | `underdamped(t, 420.0, 20.0, 9.0, 4.0)` — one visible bounce |
| `heat_intensity` | Decays 0.9 → ambient via `overdamped(t, 0.9, 8.0, 2.0)` |
| Chromatic aberration | Decays to 0 (velocity → 0) |
| Motion blur | Decays to 0 |
| Scan-line λ | Settles at 8px (idle atmospheric) or off if preferred |
| Squared-sine throb | 1-2 UT adrenaline pulses during settle: `1.0 + 0.06 * squared_sine(t, 0.4)` on heat |

```rust
fn phase_c_reveal(t_local: f32) -> f32 {
    // t_local: 0.0 to 1.0 over 130ms
    // Damped oscillation: ζ ≈ 0.4, one visible overshoot
    420.0 + 20.0 * (-4.0 * t_local).exp() * (9.0 * t_local).sin()
}
```

### Combined reveal function

```rust
struct PanelReveal {
    start: Instant,
    phase: RevealPhase,
}

enum RevealPhase { Crack, Slam, Settle, Done }

impl PanelReveal {
    const CRACK_MS: f32 = 80.0;
    const SLAM_MS: f32 = 140.0;
    const SETTLE_MS: f32 = 130.0;

    fn update(&mut self, now: Instant) -> RevealState {
        let elapsed_ms = (now - self.start).as_secs_f32() * 1000.0;

        if elapsed_ms < Self::CRACK_MS {
            let t = elapsed_ms / Self::CRACK_MS;
            RevealState {
                width: 2.0,
                heat: 1.0,
                scanline_lambda: 3.0,
                scanline_omega: 12.0,
                chromatic: 0.0,
                ghost_count: 0,
                shake: Vec2::ZERO,
            }
        } else if elapsed_ms < Self::CRACK_MS + Self::SLAM_MS {
            let t = (elapsed_ms - Self::CRACK_MS) / Self::SLAM_MS;
            let width = phase_b_reveal(t);
            let velocity = if t > 0.01 {
                (phase_b_reveal(t) - phase_b_reveal(t - 0.01)) / 0.01
            } else { 0.0 };
            RevealState {
                width,
                heat: 0.9,
                scanline_lambda: 3.0 + t * 3.0,   // 3 → 6
                scanline_omega: 12.0 - t * 6.0,    // 12 → 6
                chromatic: (velocity * 0.008).clamp(0.0, 3.0),
                ghost_count: 3,
                shake: Vec2::ZERO,
            }
        } else if elapsed_ms < Self::CRACK_MS + Self::SLAM_MS + Self::SETTLE_MS {
            let t = (elapsed_ms - Self::CRACK_MS - Self::SLAM_MS) / Self::SETTLE_MS;
            RevealState {
                width: phase_c_reveal(t),
                heat: overdamped(t, 0.9, 8.0, 2.0) + 0.06 * squared_sine(t, 0.4),
                scanline_lambda: 6.0 + t * 2.0,    // 6 → 8 (idle)
                scanline_omega: 6.0 - t * 4.0,     // 6 → 2 (slow idle)
                chromatic: 0.3 * (1.0 - t),        // residual decay
                ghost_count: if t < 0.5 { 1 } else { 0 },
                shake: Vec2::ZERO,
            }
        } else {
            self.phase = RevealPhase::Done;
            RevealState::idle()
        }
    }
}
```

---

## 2. Edge heat — "Palette-Cycled Furnace Bleed"

**Sources**: Mark Ferrari palette cycling + demoscene 1D fire automaton + Perlin wobble with Broken Sword quantized steps

### 2a. Fire automaton along Y axis

Run a 1D cellular automaton in the shader along the panel's left edge (the bleed zone). This replaces the current static glow falloff with a *living* fire column.

```wgsl
// fire_buffer: 1D texture, panel_height texels, updated each frame
// CPU-side update (or compute shader):

fn update_fire_column(buf: &mut [f32], heat_intensity: f32, rng: &mut impl Rng) {
    let h = buf.len();
    // Bottom row: inject heat
    buf[h - 1] = heat_intensity * rng.gen_range(0.7..1.0);
    buf[h - 2] = heat_intensity * rng.gen_range(0.5..1.0);

    // Propagate upward with averaging + decay
    for y in (0..h - 2).rev() {
        buf[y] = ((buf[y + 1] + buf[y] + buf[y + 2]
                   + if y + 3 < h { buf[y + 3] } else { 0.0 })
                  * 0.25 - 0.012)
                 .max(0.0);
    }
}
```

### 2b. Ferrari palette ramp with cycling

The fire automaton values [0.0, 1.0] map through a color ramp. The ramp itself
shifts over time — the Ferrari trick that makes static patterns look animated.

```wgsl
fn fire_palette(heat: f32, time: f32) -> vec4<f32> {
    // Shift the lookup by time — palette rotation
    let h = fract(heat + time * 0.08);

    // 6-stop ramp (Diablo II torch palette)
    if h < 0.15 { return vec4(0.0, 0.0, 0.0, 0.0); }                              // transparent
    if h < 0.30 { return mix(vec4(0.10, 0.0, 0.0, 0.4), vec4(0.35, 0.05, 0.0, 0.7), (h-0.15)/0.15); }
    if h < 0.50 { return mix(vec4(0.35, 0.05, 0.0, 0.7), vec4(1.0, 0.27, 0.0, 0.9), (h-0.30)/0.20); }
    if h < 0.70 { return mix(vec4(1.0, 0.27, 0.0, 0.9), vec4(1.0, 0.67, 0.0, 1.0), (h-0.50)/0.20); }
    if h < 0.88 { return mix(vec4(1.0, 0.67, 0.0, 1.0), vec4(1.0, 0.93, 0.8, 1.0), (h-0.70)/0.18); }
    return vec4(1.0, 1.0, 1.0, 1.0);                                               // white-hot
}
```

### 2c. Quantized wobble edge (Broken Sword + Perlin)

```wgsl
fn edge_displacement(frag_y: f32, time: f32, heat_intensity: f32) -> f32 {
    let noise = perlin_2d(vec2(frag_y * 0.02, time * 0.8));
    // Quantize to 2px steps — Broken Sword chunky edge aesthetic
    let stepped = floor(noise * 3.0) * 2.0;
    return stepped * heat_intensity;  // scales with activity
}
```

### 2d. Glow bleed composition

In the 20px transparent bleed zone (left edge of 440px surface), composite the
fire automaton output through the palette, displaced by the quantized wobble:

```wgsl
// Fragment shader for bleed zone (x in [0, 20])
let bleed_x = 20.0 - frag_x;  // distance from panel edge
let wobble = edge_displacement(frag_y, u_time, u_heat_intensity);
let effective_dist = bleed_x - wobble;

if effective_dist < 0.0 { discard; }

let fire_val = textureSample(fire_column_tex, samp, vec2(0.0, frag_y / panel_height)).r;
let falloff = 1.0 - smoothstep(0.0, 18.0, effective_dist);
let color = fire_palette(fire_val * falloff, u_time);
return color;
```

**Cost**: One 1D texture update per frame (CPU, trivial), one `perlin_2d` + palette lookup per bleed-zone fragment. The bleed zone is 20×panel_height pixels — negligible fragment count.

---

## 3. File indicators — dual register system

### 3a. Active write: Diablo II torch (discrete register)

8-frame sprite loop at ~8fps. Each frame is 3-4 egui painter ellipses.
Deliberately lower update rate than render loop — the choppiness signals "crafted."

```rust
const TORCH_FPS: f64 = 8.3;
const TORCH_FRAMES: usize = 8;

struct TorchFrame {
    circles: [(Vec2, f32, Color32); 4],  // offset, radius, color
}

// Precomputed frames — hand-tuned, not procedural
const FRAMES: [TorchFrame; TORCH_FRAMES] = [
    // Frame 0: base flame
    TorchFrame { circles: [
        (vec2(0.0, -2.0), 4.0, Color32::from_rgba_premultiplied(255, 100, 0, 200)),
        (vec2(0.5, -4.0), 3.0, Color32::from_rgba_premultiplied(255, 170, 0, 180)),
        (vec2(-0.3, -6.0), 2.0, Color32::from_rgba_premultiplied(255, 220, 100, 140)),
        (vec2(0.0, -8.0), 1.2, Color32::from_rgba_premultiplied(255, 255, 200, 80)),
    ]},
    // ... frames 1-7: slight Y jitter, radius variation, color shift
    // Key: frames differ by 1-2px offsets and ±20 on color channels
    // The variation is subtle but the locked framerate makes it read as fire
    // TODO: generate programmatically from a seed, then freeze as constants
];

fn draw_torch(painter: &Painter, slot_center: Pos2, time: f64, source_color: Color32) {
    let frame_idx = ((time * TORCH_FPS) as usize) % TORCH_FRAMES;
    let frame = &FRAMES[frame_idx];
    for (offset, radius, color) in &frame.circles {
        // Tint toward source color (Claude orange / Codex green)
        let tinted = tint_toward(color, source_color, 0.3);
        painter.circle_filled(slot_center + *offset, *radius, tinted);
    }
}
```

### 3b. Slot flicker: Monkey Island hotspot (discrete register)

Binary bright/dim flicker with irregular timing. NOT a sine wave.

```rust
fn flicker_intensity(time: f64, file_hash: u64) -> f32 {
    // Two incommensurate frequencies → irregular period
    let a = (time * 3.7 + file_hash as f64 * 0.1).sin();
    let b = (time * 7.1 + file_hash as f64 * 0.3).sin();
    let phase = a * b;
    if phase > 0.0 { 1.0 } else { 0.65 }  // binary states, no interpolation
}
```

### 3c. Cinder ejection on write events (continuous register)

Micro-burst of 3-6 embers in a 45° cone pointing LEFT (away from panel).
Hard cap of 24 active ember particles across all slots.

```rust
const MAX_WRITE_EMBERS: usize = 24;

fn spawn_write_burst(
    slot_y: f32,
    embers: &mut Vec<Ember>,
    rng: &mut impl Rng,
    source_heat_range: (f32, f32),  // palette heat range for this agent
) {
    let count = rng.gen_range(3..=6);
    for _ in 0..count {
        if embers.len() >= MAX_WRITE_EMBERS {
            embers.remove(0);  // kill oldest
        }
        embers.push(Ember {
            pos: vec2(0.0, slot_y),
            vel: vec2(
                -rng.gen_range(30.0..60.0),   // leftward into desktop
                rng.gen_range(-20.0..20.0),   // vertical spread
            ),
            life: 1.0,
            heat: rng.gen_range(source_heat_range.0..source_heat_range.1),
            size: 3.0,
        });
    }
}

fn update_ember(e: &mut Ember, dt: f32) {
    e.pos += e.vel * dt;
    e.vel.y -= 15.0 * dt;   // upward drift — embers rise
    e.life -= dt * 1.8;     // ~0.55s lifetime
    e.size = 1.0 + 2.0 * e.life;
    // Color: sample fire_palette(e.heat, ...) at render time
}
```

### 3d. File arrival — UT kill-stamp (continuous register)

New file entries scale 1.3→1.0 with critical damping. No overshoot — authoritative snap.

```rust
fn file_entry_scale(t_since_arrival: f32) -> f32 {
    // Critically damped: ζ = 1.0
    1.0 + 0.3 * (-6.0 * t_since_arrival).exp()
}
```

### 3e. File departure — Renegade ghost dissolution

Flash bright, then fade with concave curve (t^0.3). Simultaneous cinder burst.

```rust
struct FileDeparture {
    started: Instant,
    slot_y: f32,
    source_color: Color32,
}

impl FileDeparture {
    const DURATION: f32 = 0.6;

    fn opacity(&self, now: Instant) -> f32 {
        let t = (now - self.started).as_secs_f32();
        if t >= Self::DURATION { return 0.0; }
        concave_fade(t, Self::DURATION)
    }

    fn color(&self, now: Instant) -> Color32 {
        let t = (now - self.started).as_secs_f32();
        if t < 0.05 {
            // Initial flash: white
            Color32::from_rgba_premultiplied(255, 255, 255, 240)
        } else {
            // Fade source color with concave curve
            let a = (self.opacity(now) * 255.0) as u8;
            Color32::from_rgba_premultiplied(
                self.source_color.r(), self.source_color.g(), self.source_color.b(), a
            )
        }
    }
}
```

---

## 4. Agent connection — FlatOut impact shake

When a new agent session connects (Claude or Codex), the entire panel shudders.

```rust
struct AgentShake {
    start: Instant,
    amplitude: f32,   // 4.0px
    decay: f32,       // 8.0 (visible ~300ms)
    seed: u64,
}

impl AgentShake {
    fn margin_offset(&self, now: Instant) -> (i32, i32) {
        let t = (now - self.start).as_secs_f32();
        let offset = shake_offset(t, self.amplitude, self.decay, 25.0, self.seed);
        (offset.x.round() as i32, offset.y.round() as i32)
    }
}

// In the main loop, apply to layer_surface margins:
fn apply_shake(base_margin: i32, shake: Option<&AgentShake>, now: Instant) -> (i32, i32, i32, i32) {
    let (dx, dy) = shake.map(|s| s.margin_offset(now)).unwrap_or((0, 0));
    (
        8 + dy,      // top margin + shake Y
        8 + dx,      // right margin + shake X
        8 - dy,      // bottom margin - shake Y (opposite to preserve height)
        0,           // left margin unchanged
    )
}
```

Simultaneous with shake: `heat_intensity` spikes to 1.0 and decays via overdamped.

---

## 5. Idle state ambient effects

### 5a. Activity-scaled pulse shape

Low activity (< 0.3): No pulse. Static warm glow.
Medium activity (0.3–0.7): Gentle sine breathing, period 2.0s.
High activity (> 0.7): **Squared-sine throb** (UT2004 adrenaline model), period 1.2s.

```rust
fn idle_pulse(heat_intensity: f32, time: f64) -> f32 {
    if heat_intensity < 0.3 {
        0.0
    } else if heat_intensity < 0.7 {
        let strength = (heat_intensity - 0.3) / 0.4;  // 0→1 in range
        strength * 0.04 * (time as f32 * std::f32::consts::PI).sin()
    } else {
        let strength = (heat_intensity - 0.7) / 0.3;
        strength * 0.08 * squared_sine(time as f32, 1.2)
    }
}
```

### 5b. Idle scan-lines (Renegade atmospheric)

When scan-lines are enabled (post-entrance), maintain at λ=8px, ω=2.0 (slow drift).
At high activity, tighten to λ=5px, ω=4.0 — "the display is running hotter."

```rust
fn idle_scanline_params(heat_intensity: f32) -> (f32, f32) {
    let lambda = 8.0 - heat_intensity * 3.0;  // 8 → 5
    let omega = 2.0 + heat_intensity * 2.0;   // 2 → 4
    (lambda, omega)
}
```

### 5c. Ferrari palette drift

Continuous palette cycling at rate `time * 0.08` (in `fire_palette()` above).
This is always active. Zero cost — it's a uniform that shifts the palette lookup.

---

## 6. Implementation priority

| Phase | What | Dependencies | Est. effort |
|---|---|---|---|
| **1** | Panel reveal (3-phase ignition) | New `PanelReveal` state machine, `u_reveal_width` uniform | Medium — new state machine + shader uniform |
| **1** | Heat spike on open | One-liner in toggle handler | Trivial |
| **2** | Scan-line mask in shader | New `u_scanline_lambda`, `u_scanline_omega` uniforms | Small — ~10 lines WGSL |
| **2** | Chromatic aberration | New `u_chromatic_offset` uniform, 3-tap sample in composite | Small — ~15 lines WGSL |
| **3** | Fire automaton + Ferrari palette | 1D texture, CPU update loop, palette function in WGSL | Medium — new texture pipeline |
| **3** | Quantized wobble edge | `perlin_2d` in WGSL (or precomputed noise texture) | Small if noise texture, medium if inline Perlin |
| **4** | Torch sprites for active writes | `TorchFrame` constants, egui painter calls | Small — pure egui, no GPU |
| **4** | Flicker intensity | One function, applied to existing slot color | Trivial |
| **4** | Cinder ejection bursts | Extend existing particle spawn with write positions | Small — reuse particle system |
| **5** | File arrival stamp / departure ghost | `file_entry_scale()`, `FileDeparture` struct | Small — pure egui |
| **5** | Agent connect shake | `AgentShake` struct, margin jitter | Small — Wayland protocol only |
| **6** | Idle pulse shape + squared-sine | Replace current pulse logic | Trivial |
| **6** | Motion blur ghosts | Multi-sample in composite shader | Small — ~10 lines WGSL |

## Uniform budget (all passed to `heat_glow.wgsl` or composite pass)

```rust
struct AnimationUniforms {
    time: f32,
    reveal_width: f32,        // 0..440, controls clip
    heat_intensity: f32,      // 0..1, drives fire + glow
    scanline_lambda: f32,     // px spacing, 3..8
    scanline_omega: f32,      // scroll rate, 2..12
    chromatic_offset: f32,    // px, 0..3
    ghost_count: u32,         // 0..3, motion blur copies
    palette_shift: f32,       // Ferrari cycling offset (= time * 0.08)
}
```

8 uniforms. One uniform buffer update per frame. This is the entire animation
system's interface with the GPU.
