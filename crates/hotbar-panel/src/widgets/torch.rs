//! Torch sprites and cinder ejection for active-write file indicators.
//!
//! **Torch**: 8-frame sprite loop at ~8fps drawn with egui painter ellipses.
//! Deliberately lower framerate than the render loop -- the choppiness signals
//! "crafted, intentional" (discrete animation register, a la Diablo II torches).
//!
//! **Cinder**: Micro-burst of 3-6 embers spawned on write events, flying
//! leftward (away from panel into the bleed zone) with upward drift.
//! Hard cap of 24 active ember particles across all file slots.
//!
//! **Flicker**: Binary bright/dim modulation for active-write file slots,
//! driven by two incommensurate sine frequencies for irregular timing.

use std::sync::LazyLock;

use egui::{Color32, Painter, Pos2};

// ── Torch Sprite ─────────────────────────────────────────────────────

/// Torch animation framerate (deliberately choppy -- discrete register).
const TORCH_FPS: f32 = 8.3;

/// Number of frames in the torch sprite loop.
const TORCH_FRAME_COUNT: usize = 8;

/// Number of circles per torch frame.
const CIRCLES_PER_FRAME: usize = 4;

/// A single circle in a torch animation frame.
#[derive(Debug, Clone, Copy)]
struct TorchCircle {
    offset_x: f32,
    offset_y: f32,
    radius: f32,
    color: Color32,
}

/// Precomputed torch sprite frames.
///
/// Each frame is 4 layered circles: base (orange), body (yellow-orange),
/// tip (bright yellow), and spark (near-white, tiny). Frames differ by
/// 1-2px offsets and subtle radius/color variation. The locked 8fps
/// framerate makes the variation read as flickering fire.
static TORCH_FRAMES: LazyLock<[[TorchCircle; CIRCLES_PER_FRAME]; TORCH_FRAME_COUNT]> =
    LazyLock::new(generate_torch_frames);

fn generate_torch_frames() -> [[TorchCircle; CIRCLES_PER_FRAME]; TORCH_FRAME_COUNT] {
    // Per-frame jitter offsets (hand-tuned, not random)
    let jitter: [(f32, f32); TORCH_FRAME_COUNT] = [
        (0.0, 0.0),
        (0.5, -0.3),
        (-0.3, 0.5),
        (0.7, 0.2),
        (-0.5, -0.5),
        (0.2, 0.7),
        (-0.7, 0.0),
        (0.3, -0.6),
    ];

    let default = TorchCircle {
        offset_x: 0.0,
        offset_y: 0.0,
        radius: 0.0,
        color: Color32::TRANSPARENT,
    };
    let mut frames = [[default; CIRCLES_PER_FRAME]; TORCH_FRAME_COUNT];

    for (i, &(jx, jy)) in jitter.iter().enumerate() {
        let phase = i as f32;
        // Base flame -- large, deep orange
        frames[i][0] = TorchCircle {
            offset_x: jx * 0.8,
            offset_y: -2.0 + jy * 0.5,
            radius: 4.0 + (phase * 0.3).sin() * 0.5,
            color: Color32::from_rgba_premultiplied(255, 100, 0, 200),
        };
        // Body -- medium, yellow-orange
        frames[i][1] = TorchCircle {
            offset_x: 0.5 + jx,
            offset_y: -4.0 + jy * 0.7,
            radius: 3.0 + (phase * 0.7).cos() * 0.4,
            color: Color32::from_rgba_premultiplied(255, 170, 0, 180),
        };
        // Tip -- small, bright yellow
        frames[i][2] = TorchCircle {
            offset_x: -0.3 + jx * 0.6,
            offset_y: -6.0 + jy,
            radius: 2.0 + (phase * 1.1).sin() * 0.3,
            color: Color32::from_rgba_premultiplied(255, 220, 100, 140),
        };
        // Spark -- tiny, near-white (topmost point)
        frames[i][3] = TorchCircle {
            offset_x: jx * 0.4,
            offset_y: -8.0 + jy * 0.8,
            radius: 1.2 + (phase * 1.5).cos() * 0.2,
            color: Color32::from_rgba_premultiplied(255, 255, 200, 80),
        };
    }

    frames
}

/// Draw a torch sprite at the given position.
///
/// `center`: Position of the file slot's source indicator dot.
/// `time`: Elapsed time in seconds (drives frame selection at 8.3fps).
/// `source_color`: Agent source color (flame is tinted 30% toward this).
pub fn draw_torch(painter: &Painter, center: Pos2, time: f32, source_color: Color32) {
    crate::dev_trace_span!("torch_sprite");
    let frame_idx = ((time * TORCH_FPS) as usize) % TORCH_FRAME_COUNT;
    let frame = &TORCH_FRAMES[frame_idx];

    for circle in frame {
        let tinted = tint_rgb(circle.color, source_color, 0.3);
        painter.circle_filled(
            Pos2::new(center.x + circle.offset_x, center.y + circle.offset_y),
            circle.radius,
            tinted,
        );
    }
}

/// Tint a color's RGB channels toward a target, preserving the original alpha.
fn tint_rgb(base: Color32, target: Color32, factor: f32) -> Color32 {
    let inv = 1.0 - factor;
    Color32::from_rgba_premultiplied(
        (base.r() as f32 * inv + target.r() as f32 * factor) as u8,
        (base.g() as f32 * inv + target.g() as f32 * factor) as u8,
        (base.b() as f32 * inv + target.b() as f32 * factor) as u8,
        base.a(),
    )
}

// ── Cinder System ────────────────────────────────────────────────────

/// Maximum active ember particles across all file slots.
const MAX_EMBERS: usize = 24;

/// A single cinder ember particle.
#[derive(Debug, Clone, Copy)]
struct Ember {
    /// Position relative to panel origin (x=0 is panel left edge)
    pos_x: f32,
    pos_y: f32,
    vel_x: f32,
    vel_y: f32,
    /// Remaining life (1.0 -> 0.0 over ~0.55s)
    life: f32,
    /// Color temperature (0.0 = dark red, 1.0 = white-hot)
    heat: f32,
}

/// CPU-side cinder particle system drawn via egui `Painter`.
///
/// Spawns micro-bursts of 3-6 embers on write events. Embers fly leftward
/// (away from panel) with upward drift, ~0.55s lifetime. Hard cap of 24 particles.
///
/// Uses a fixed-size array with swap-remove (same pattern as FlamePass)
/// — zero heap allocation after construction.
pub struct CinderSystem {
    embers: [Ember; MAX_EMBERS],
    /// Number of live embers in `embers[..active]`.
    active: usize,
    /// Monotonic counter for deterministic spawn variation.
    counter: u32,
}

const DEAD_EMBER: Ember = Ember {
    pos_x: 0.0,
    pos_y: 0.0,
    vel_x: 0.0,
    vel_y: 0.0,
    life: 0.0,
    heat: 0.0,
};

impl CinderSystem {
    /// Create an empty cinder system.
    pub fn new() -> Self {
        Self {
            embers: [DEAD_EMBER; MAX_EMBERS],
            active: 0,
            counter: 0,
        }
    }

    /// Spawn a burst of cinder embers at the given slot Y position.
    ///
    /// Called when a write event is detected for a file.
    /// `slot_y`: Y coordinate of the file's slot (panel-relative).
    /// `heat`: Color temperature of embers (0.0-1.0, tied to source intensity).
    pub fn spawn_burst(&mut self, slot_y: f32, heat: f32) {
        let count = 3 + (self.hash_next() % 4) as usize; // 3-6 embers

        for _ in 0..count {
            if self.active >= MAX_EMBERS {
                // Evict coldest (lowest life) via swap-remove
                let mut coldest = 0;
                for i in 1..self.active {
                    if self.embers[i].life < self.embers[coldest].life {
                        coldest = i;
                    }
                }
                self.active -= 1;
                self.embers.swap(coldest, self.active);
            }

            let h1 = self.hash_next();
            let h2 = self.hash_next();
            let h3 = self.hash_next();

            self.embers[self.active] = Ember {
                pos_x: 0.0, // panel left edge
                pos_y: slot_y,
                vel_x: -(30.0 + (h1 % 30) as f32),   // leftward into bleed zone
                vel_y: -20.0 + (h2 % 40) as f32,      // vertical spread +-20
                life: 1.0,
                heat: heat.clamp(0.0, 1.0) * (0.7 + (h3 % 30) as f32 * 0.01),
            };
            self.active += 1;
        }
    }

    /// Advance all ember physics by `dt` seconds. Call once per frame.
    pub fn update(&mut self, dt: f32) {
        crate::dev_trace_span!("cinder_update");
        for i in 0..self.active {
            self.embers[i].pos_x += self.embers[i].vel_x * dt;
            self.embers[i].pos_y += self.embers[i].vel_y * dt;
            self.embers[i].vel_y -= 15.0 * dt; // upward drift -- embers rise
            self.embers[i].life -= dt * 1.8;    // ~0.55s lifetime
        }

        // Swap-remove dead embers (same pattern as FlamePass)
        let mut i = 0;
        while i < self.active {
            if self.embers[i].life <= 0.0 {
                self.active -= 1;
                self.embers.swap(i, self.active);
            } else {
                i += 1;
            }
        }
    }

    /// Draw all active embers with the egui painter.
    ///
    /// `origin`: top-left of the panel in screen coordinates.
    pub fn draw(&self, painter: &Painter, origin: Pos2) {
        crate::dev_trace_span!("cinder_draw");
        for i in 0..self.active {
            let ember = &self.embers[i];
            let alpha = (ember.life.clamp(0.0, 1.0) * 220.0) as u8;
            let color = ember_color(ember.heat, alpha);
            let size = 1.0 + 2.0 * ember.life.max(0.0);
            let pos = Pos2::new(origin.x + ember.pos_x, origin.y + ember.pos_y);
            painter.circle_filled(pos, size, color);
        }
    }

    /// Spawn a single ember at a specific Y position.
    ///
    /// Lighter than `spawn_burst` — used for passive fire-driven ejection.
    /// Does not evict existing embers; silently drops if at capacity.
    pub fn spawn_at(&mut self, y: f32, heat: f32) {
        if self.active >= MAX_EMBERS {
            return;
        }
        let h = self.hash_next();
        self.embers[self.active] = Ember {
            pos_x: 0.0,
            pos_y: y,
            vel_x: -(20.0 + (h % 20) as f32),
            vel_y: -10.0 + (h % 20) as f32,
            life: 0.6,
            heat: heat.clamp(0.0, 1.0),
        };
        self.active += 1;
    }

    /// Number of active embers (for diagnostics).
    pub fn active_count(&self) -> usize {
        self.active
    }

    /// Deterministic hash for spawn variation (not cryptographic -- just variety).
    fn hash_next(&mut self) -> u32 {
        self.counter = self.counter.wrapping_add(1);
        let mut h = self.counter.wrapping_mul(2654435761);
        h ^= h >> 16;
        h
    }
}

impl Default for CinderSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Map ember heat + alpha to a fire palette color.
///
/// 3-stop ramp: dark red → orange → yellow-white, matching the
/// Diablo II torch palette.
fn ember_color(heat: f32, alpha: u8) -> Color32 {
    if heat < 0.3 {
        // Dark ember -- deep red
        let t = heat / 0.3;
        Color32::from_rgba_premultiplied((t * 180.0) as u8, 0, 0, alpha / 2)
    } else if heat < 0.6 {
        // Warm ember -- orange
        let t = (heat - 0.3) / 0.3;
        Color32::from_rgba_premultiplied(
            180 + (t * 75.0) as u8,
            (t * 120.0) as u8,
            0,
            alpha,
        )
    } else {
        // Hot ember -- yellow-white
        let t = (heat - 0.6) / 0.4;
        Color32::from_rgba_premultiplied(
            255,
            120 + (t * 135.0) as u8,
            (t * 100.0) as u8,
            alpha,
        )
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Compute a simple hash from a file path (for flicker seed).
pub fn path_hash(path: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV offset basis
    for b in path.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    h
}

/// Whether a file should show the torch sprite (active write indicator).
///
/// Returns true for Modified or Created actions. The recency check is
/// left to the caller (typically: timestamp within last 10s).
pub fn is_active_write(action: hotbar_common::Action) -> bool {
    matches!(
        action,
        hotbar_common::Action::Modified | hotbar_common::Action::Created
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torch_frames_are_valid() {
        let frames = &*TORCH_FRAMES;
        assert_eq!(frames.len(), TORCH_FRAME_COUNT);
        for frame in frames {
            assert_eq!(frame.len(), CIRCLES_PER_FRAME);
            // All circles should have positive radius
            for circle in frame {
                assert!(circle.radius > 0.0, "radius must be positive");
            }
        }
    }

    #[test]
    fn torch_frames_differ() {
        let frames = &*TORCH_FRAMES;
        // Frames should not all be identical (the jitter makes them differ)
        let f0 = frames[0][0].offset_x;
        let f1 = frames[1][0].offset_x;
        assert_ne!(f0, f1, "frames should differ");
    }

    #[test]
    fn torch_frame_selection_wraps() {
        // At 8.3fps, frame index should cycle through 0-7
        for i in 0..100 {
            let time = i as f32 * 0.05;
            let idx = ((time * TORCH_FPS) as usize) % TORCH_FRAME_COUNT;
            assert!(idx < TORCH_FRAME_COUNT);
        }
    }

    #[test]
    fn tint_rgb_preserves_alpha() {
        let base = Color32::from_rgba_premultiplied(255, 0, 0, 128);
        let target = Color32::from_rgba_premultiplied(0, 255, 0, 255);
        let result = tint_rgb(base, target, 0.5);
        assert_eq!(result.a(), 128, "alpha should be preserved from base");
    }

    #[test]
    fn tint_rgb_zero_factor_is_base() {
        let base = Color32::from_rgba_premultiplied(100, 50, 25, 200);
        let target = Color32::from_rgba_premultiplied(0, 0, 0, 0);
        let result = tint_rgb(base, target, 0.0);
        assert_eq!(result.r(), 100);
        assert_eq!(result.g(), 50);
        assert_eq!(result.b(), 25);
    }

    #[test]
    fn cinder_spawn_burst_creates_embers() {
        let mut sys = CinderSystem::new();
        sys.spawn_burst(500.0, 0.8);
        assert!(sys.active_count() >= 3);
        assert!(sys.active_count() <= 6);
    }

    #[test]
    fn cinder_embers_die_over_time() {
        let mut sys = CinderSystem::new();
        sys.spawn_burst(500.0, 0.8);
        let initial = sys.active_count();
        assert!(initial > 0);

        // Advance well past lifetime (~0.55s)
        for _ in 0..20 {
            sys.update(0.05);
        }
        assert_eq!(sys.active_count(), 0, "all embers should expire");
    }

    #[test]
    fn cinder_cap_at_max() {
        let mut sys = CinderSystem::new();
        // Spawn many bursts
        for i in 0..20 {
            sys.spawn_burst(i as f32 * 50.0, 0.5);
        }
        assert!(
            sys.active_count() <= MAX_EMBERS,
            "should never exceed cap: {}",
            sys.active_count()
        );
    }

    #[test]
    fn cinder_embers_move_leftward() {
        let mut sys = CinderSystem::new();
        sys.spawn_burst(500.0, 0.8);
        let active = sys.active_count();
        let initial_x: Vec<f32> = sys.embers[..active].iter().map(|e| e.pos_x).collect();

        sys.update(0.1);
        for (i, ember) in sys.embers[..active].iter().enumerate() {
            assert!(
                ember.pos_x < initial_x[i],
                "embers should move leftward (away from panel)"
            );
        }
    }

    #[test]
    fn ember_color_dark_red_at_low_heat() {
        let c = ember_color(0.1, 200);
        assert!(c.r() > 0);
        assert_eq!(c.g(), 0);
        assert_eq!(c.b(), 0);
    }

    #[test]
    fn ember_color_orange_at_mid_heat() {
        let c = ember_color(0.5, 200);
        assert!(c.r() > 200);
        assert!(c.g() > 0);
        assert_eq!(c.b(), 0);
    }

    #[test]
    fn ember_color_bright_at_high_heat() {
        let c = ember_color(0.9, 200);
        assert_eq!(c.r(), 255);
        assert!(c.g() > 150);
        assert!(c.b() > 0);
    }

    #[test]
    fn path_hash_deterministic() {
        let h1 = path_hash("/home/user/src/main.rs");
        let h2 = path_hash("/home/user/src/main.rs");
        assert_eq!(h1, h2);
    }

    #[test]
    fn path_hash_varies() {
        let h1 = path_hash("/home/user/a.rs");
        let h2 = path_hash("/home/user/b.rs");
        assert_ne!(h1, h2);
    }

    #[test]
    fn is_active_write_correct() {
        assert!(is_active_write(hotbar_common::Action::Modified));
        assert!(is_active_write(hotbar_common::Action::Created));
        assert!(!is_active_write(hotbar_common::Action::Opened));
        assert!(!is_active_write(hotbar_common::Action::Deleted));
    }

    #[test]
    fn spawn_at_creates_one_ember() {
        let mut sys = CinderSystem::new();
        sys.spawn_at(100.0, 0.6);
        assert_eq!(sys.active_count(), 1);
    }

    #[test]
    fn spawn_at_respects_cap() {
        let mut sys = CinderSystem::new();
        // Fill to capacity via bursts
        for i in 0..20 {
            sys.spawn_burst(i as f32 * 50.0, 0.5);
        }
        assert_eq!(sys.active_count(), MAX_EMBERS);

        // spawn_at should silently drop, not evict
        sys.spawn_at(500.0, 0.8);
        assert_eq!(sys.active_count(), MAX_EMBERS);
    }
}
