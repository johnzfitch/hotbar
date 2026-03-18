//! Animation primitives and state machines.
//!
//! All curves derive from damped harmonic oscillator solutions.
//! Two animation registers: discrete (crafted, authorial) and
//! continuous (physical, mathematical).

use std::time::{Duration, Instant};

// ── Math Primitives ──────────────────────────────────────────────────

/// Underdamped spring (zeta ~ 0.4) -- overshoots and rocks.
/// For heavy mechanical elements (panel slam).
pub fn underdamped(t: f32, target: f32, overshoot: f32, freq: f32, decay: f32) -> f32 {
    target + overshoot * (-decay * t).exp() * (freq * t).sin()
}

/// Critically damped (zeta = 1.0) -- snaps into place, no oscillation.
/// For text/data stamps (file arrival).
pub fn critically_damped(t: f32, initial_offset: f32, rate: f32) -> f32 {
    1.0 + initial_offset * (-rate * t).exp()
}

/// Overdamped (zeta > 1) -- sluggish settle.
/// For ambient glow decay after spikes.
pub fn overdamped(t: f32, amplitude: f32, fast_decay: f32, slow_decay: f32) -> f32 {
    amplitude * (0.6 * (-fast_decay * t).exp() + 0.4 * (-slow_decay * t).exp())
}

/// Squared sine -- sharper peak, longer trough.
/// UT2004 adrenaline pulse shape. Reads as "heartbeat."
pub fn squared_sine(t: f32, period: f32) -> f32 {
    let s = (std::f32::consts::PI * t / period).sin();
    s * s
}

/// Concave fade -- fast initial drop, long ghostly tail.
/// Exponent 0.3 gives "ghost lingers" quality.
pub fn concave_fade(t: f32, duration: f32) -> f32 {
    let norm = (t / duration).clamp(0.0, 1.0);
    1.0 - norm.powf(0.3)
}

/// Cubic ease-out: fast start, smooth deceleration.
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

// ── Panel Reveal ─────────────────────────────────────────────────────

/// Reveal animation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevealPhase {
    /// 2px slit, max heat, CRT flicker (0-80ms)
    Crack,
    /// Slam open with cubic ease-out (80-220ms)
    Slam,
    /// Underdamped oscillation settle (220-350ms)
    Settle,
    /// Animation complete, normal rendering
    Done,
    /// Panel is hidden (no rendering)
    Hidden,
}

/// Per-frame output from the reveal state machine.
#[derive(Debug, Clone, Copy)]
pub struct RevealState {
    /// Visible panel width in pixels (0..440 for 420px panel + 20px bleed)
    pub width: f32,
    /// Heat intensity override (None = use daemon value)
    pub heat_override: Option<f32>,
    /// Scan-line wavelength in pixels
    pub scanline_lambda: f32,
    /// Scan-line scroll rate
    pub scanline_omega: f32,
    /// Current phase
    pub phase: RevealPhase,
}

impl RevealState {
    /// Normal idle state (no reveal animation active).
    pub fn idle() -> Self {
        Self {
            width: f32::MAX, // no clipping
            heat_override: None,
            scanline_lambda: 8.0,
            scanline_omega: 2.0,
            phase: RevealPhase::Done,
        }
    }

    /// Hidden state.
    pub fn hidden() -> Self {
        Self {
            width: 0.0,
            heat_override: Some(0.0),
            scanline_lambda: 3.0,
            scanline_omega: 0.0,
            phase: RevealPhase::Hidden,
        }
    }
}

/// Panel reveal state machine.
///
/// Three-phase entrance: Crack (2px slit, max heat) -> Slam (cubic ease-out)
/// -> Settle (underdamped oscillation).
pub struct PanelReveal {
    start: Option<Instant>,
    phase: RevealPhase,
    /// Full panel width including bleed zone
    panel_width: f32,
}

impl PanelReveal {
    const CRACK_MS: f32 = 80.0;
    const SLAM_MS: f32 = 140.0;
    const SETTLE_MS: f32 = 130.0;
    const CRACK_WIDTH: f32 = 2.0;

    /// Create a new reveal controller.
    pub fn new(panel_width: f32) -> Self {
        Self {
            start: None,
            phase: RevealPhase::Done,
            panel_width,
        }
    }

    /// Trigger the reveal animation (panel opening).
    pub fn trigger_open(&mut self) {
        self.start = Some(Instant::now());
        self.phase = RevealPhase::Crack;
        tracing::debug!("panel reveal: ignition");
    }

    /// Immediately hide (panel closing -- no exit animation yet).
    pub fn trigger_close(&mut self) {
        self.start = None;
        self.phase = RevealPhase::Hidden;
    }

    /// Whether the panel is visible (any phase except Hidden).
    pub fn is_visible(&self) -> bool {
        self.phase != RevealPhase::Hidden
    }

    /// Whether the reveal animation is still running.
    pub fn is_animating(&self) -> bool {
        matches!(self.phase, RevealPhase::Crack | RevealPhase::Slam | RevealPhase::Settle)
    }

    /// Current phase.
    pub fn phase(&self) -> RevealPhase {
        self.phase
    }

    /// Update panel width (on surface resize).
    pub fn set_panel_width(&mut self, width: f32) {
        self.panel_width = width;
    }

    /// Compute the current reveal state.
    pub fn update(&mut self) -> RevealState {
        let Some(start) = self.start else {
            return if self.phase == RevealPhase::Hidden {
                RevealState::hidden()
            } else {
                RevealState::idle()
            };
        };

        let elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
        let total = Self::CRACK_MS + Self::SLAM_MS + Self::SETTLE_MS;

        if elapsed_ms >= total {
            self.phase = RevealPhase::Done;
            self.start = None;
            return RevealState::idle();
        }

        if elapsed_ms < Self::CRACK_MS {
            // Phase A: Crack -- 2px slit, furnace heat, tight scan-lines
            self.phase = RevealPhase::Crack;
            RevealState {
                width: Self::CRACK_WIDTH,
                heat_override: Some(1.0),
                scanline_lambda: 3.0,
                scanline_omega: 12.0,
                phase: RevealPhase::Crack,
            }
        } else if elapsed_ms < Self::CRACK_MS + Self::SLAM_MS {
            // Phase B: Slam open -- cubic ease-out
            self.phase = RevealPhase::Slam;
            let t = (elapsed_ms - Self::CRACK_MS) / Self::SLAM_MS;
            let width = Self::CRACK_WIDTH + ease_out_cubic(t) * (self.panel_width - Self::CRACK_WIDTH);
            RevealState {
                width,
                heat_override: Some(0.9),
                scanline_lambda: 3.0 + t * 3.0,  // 3 -> 6
                scanline_omega: 12.0 - t * 6.0,   // 12 -> 6
                phase: RevealPhase::Slam,
            }
        } else {
            // Phase C: Settle -- underdamped oscillation
            self.phase = RevealPhase::Settle;
            let t = (elapsed_ms - Self::CRACK_MS - Self::SLAM_MS) / Self::SETTLE_MS;
            let width = underdamped(t, self.panel_width, 20.0, 9.0, 4.0);
            let heat = overdamped(t, 0.9, 8.0, 2.0) + 0.06 * squared_sine(t, 0.4);
            RevealState {
                width: width.max(0.0),
                heat_override: Some(heat),
                scanline_lambda: 6.0 + t * 2.0,   // 6 -> 8 (idle)
                scanline_omega: 6.0 - t * 4.0,     // 6 -> 2 (slow idle)
                phase: RevealPhase::Settle,
            }
        }
    }
}

// ── Burn-In Mitigation ───────────────────────────────────────────────

/// OLED burn-in mitigation via periodic subpixel content shift.
///
/// Shifts the entire panel's render content by 1-2px every 5 minutes,
/// imperceptibly. Distributes phosphor/OLED wear across adjacent pixels.
/// Same technique as Samsung Always-On Display.
pub struct BurnInMitigation {
    last_shift: Instant,
    offset: egui::Vec2,
    /// Monotonic counter for deterministic hash
    shift_count: u64,
}

impl BurnInMitigation {
    const SHIFT_INTERVAL: Duration = Duration::from_secs(300); // 5 min
    const MAX_SHIFT: f32 = 2.0;

    /// Create a new mitigation controller (starts with zero offset).
    pub fn new() -> Self {
        Self {
            last_shift: Instant::now(),
            offset: egui::Vec2::ZERO,
            shift_count: 0,
        }
    }

    /// Get the current offset. Call once per frame.
    ///
    /// Returns a subpixel offset to apply to the egui paint origin.
    /// Changes every 5 minutes.
    pub fn update(&mut self) -> egui::Vec2 {
        let now = Instant::now();
        if now.duration_since(self.last_shift) >= Self::SHIFT_INTERVAL {
            self.last_shift = now;
            self.shift_count = self.shift_count.wrapping_add(1);
            // Deterministic hash from counter -- reproducible, not RNG
            let h1 = self.shift_count.wrapping_mul(2654435761);
            let h2 = self.shift_count.wrapping_mul(2246822519);
            self.offset = egui::vec2(
                ((h1 & 0xFF) as f32 / 255.0 - 0.5) * Self::MAX_SHIFT * 2.0,
                ((h2 & 0xFF) as f32 / 255.0 - 0.5) * Self::MAX_SHIFT * 2.0,
            );
            tracing::debug!(
                x = self.offset.x,
                y = self.offset.y,
                "burn-in mitigation: shifted content"
            );
        }
        self.offset
    }

    /// Current offset without updating.
    pub fn offset(&self) -> egui::Vec2 {
        self.offset
    }
}

impl Default for BurnInMitigation {
    fn default() -> Self {
        Self::new()
    }
}

// ── Idle Pulse ───────────────────────────────────────────────────────

/// Activity-scaled idle pulse. Replaces constant heat with living throb.
///
/// Low (< 0.3): No pulse.
/// Medium (0.3-0.7): Gentle sine breathing, 2s period.
/// High (> 0.7): Squared-sine throb (UT2004 adrenaline), 1.2s period.
pub fn idle_pulse(heat_intensity: f32, time: f32) -> f32 {
    if heat_intensity < 0.3 {
        0.0
    } else if heat_intensity < 0.7 {
        let strength = (heat_intensity - 0.3) / 0.4;
        strength * 0.04 * (time * std::f32::consts::PI).sin()
    } else {
        let strength = (heat_intensity - 0.7) / 0.3;
        strength * 0.08 * squared_sine(time, 1.2)
    }
}

// ── File Entry Animation ─────────────────────────────────────────────

/// Scale factor for a newly arrived file entry.
/// Critically damped: 1.3x -> 1.0x with no overshoot.
pub fn file_entry_scale(t_since_arrival: f32) -> f32 {
    1.0 + 0.3 * (-6.0 * t_since_arrival).exp()
}

/// Binary flicker intensity for active-write files.
/// Two incommensurate frequencies -> irregular period.
/// NOT a sine wave -- binary bright/dim states.
pub fn flicker_intensity(time: f32, file_hash: u64) -> f32 {
    let seed = file_hash as f32;
    let a = (time * 3.7 + seed * 0.1).sin();
    let b = (time * 7.1 + seed * 0.3).sin();
    if a * b > 0.0 { 1.0 } else { 0.65 }
}

// ── Agent Shake ─────────────────────────────────────────────────────

/// Agent shake — horizontal panel oscillation on heavy write activity.
///
/// Triggers when heat_intensity spikes above a threshold. Produces a
/// decaying horizontal oscillation that communicates "the system is
/// under heavy write load," like a gauge needle slamming against the stop.
pub struct AgentShake {
    /// Current shake amplitude (0.0..1.0, decays to 0)
    intensity: f32,
    /// Phase accumulator (radians)
    phase: f32,
    /// Previous frame's heat intensity (for spike detection)
    prev_heat: f32,
}

impl AgentShake {
    /// Heat threshold above which spikes can trigger shake.
    const HEAT_THRESHOLD: f32 = 0.6;
    /// Minimum heat delta (frame-over-frame) to trigger a shake.
    const SPIKE_DELTA: f32 = 0.15;
    /// Maximum horizontal displacement in pixels.
    const MAX_PX: f32 = 5.0;
    /// Oscillation frequency in Hz.
    const FREQ_HZ: f32 = 15.0;
    /// Exponential decay rate (per second).
    const DECAY_RATE: f32 = 8.0;

    /// Create a new shake controller.
    pub fn new() -> Self {
        Self {
            intensity: 0.0,
            phase: 0.0,
            prev_heat: 0.0,
        }
    }

    /// Advance the shake and return the horizontal pixel offset.
    ///
    /// Call once per frame. Returns 0.0 when no shake is active.
    pub fn update(&mut self, dt: f32, heat_intensity: f32) -> f32 {
        crate::dev_trace_span!("agent_shake");

        // Detect heat spike (rising edge only)
        let delta = heat_intensity - self.prev_heat;
        if heat_intensity > Self::HEAT_THRESHOLD && delta > Self::SPIKE_DELTA {
            self.intensity = (self.intensity + delta * 2.0).min(1.0);
            tracing::debug!(
                heat = heat_intensity,
                delta = delta,
                "agent shake triggered"
            );
        }
        self.prev_heat = heat_intensity;

        // Decay
        self.intensity = (self.intensity - dt * Self::DECAY_RATE).max(0.0);
        if self.intensity < 0.001 {
            self.phase = 0.0;
            return 0.0;
        }

        // Advance phase
        self.phase += dt * Self::FREQ_HZ * std::f32::consts::TAU;

        self.intensity * Self::MAX_PX * self.phase.sin()
    }

    /// Whether a shake is currently active.
    pub fn is_active(&self) -> bool {
        self.intensity > 0.001
    }
}

impl Default for AgentShake {
    fn default() -> Self {
        Self::new()
    }
}

// ── File Transition ─────────────────────────────────────────────────

/// Direction of a file transition animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    /// File is sliding into the spinner from the left.
    Arriving,
    /// File is fading out of the spinner to the right.
    Departing,
}

/// Visual state for a file transitioning in or out of the spinner.
///
/// Arrival: slide from left + fade in (ease-out cubic, ~0.3s).
/// Departure: fade out + slide right (~0.25s).
#[derive(Debug, Clone, Copy)]
pub struct FileTransition {
    /// Seconds since transition started.
    pub elapsed: f32,
    /// Whether this is an arrival or departure.
    pub kind: TransitionKind,
}

impl FileTransition {
    /// Duration of arrival animation in seconds.
    pub const ARRIVAL_DURATION: f32 = 0.3;
    /// Duration of departure animation in seconds.
    pub const DEPARTURE_DURATION: f32 = 0.25;

    /// Create a new arrival transition at elapsed=0.
    pub fn arrival() -> Self {
        Self { elapsed: 0.0, kind: TransitionKind::Arriving }
    }

    /// Create an arrival transition at a given elapsed time.
    pub fn arrival_at(elapsed: f32) -> Self {
        Self { elapsed, kind: TransitionKind::Arriving }
    }

    /// Create a new departure transition at elapsed=0.
    pub fn departure() -> Self {
        Self { elapsed: 0.0, kind: TransitionKind::Departing }
    }

    /// Create a departure transition at a given elapsed time.
    pub fn departure_at(elapsed: f32) -> Self {
        Self { elapsed, kind: TransitionKind::Departing }
    }

    /// Horizontal pixel offset (applied to file slot X position).
    ///
    /// Arrival: slides from -50px to 0px.
    /// Departure: slides from 0px to +30px.
    pub fn x_offset(&self) -> f32 {
        match self.kind {
            TransitionKind::Arriving => {
                let t = (self.elapsed / Self::ARRIVAL_DURATION).clamp(0.0, 1.0);
                -50.0 * (1.0 - ease_out_cubic(t))
            }
            TransitionKind::Departing => {
                let t = (self.elapsed / Self::DEPARTURE_DURATION).clamp(0.0, 1.0);
                30.0 * t * t
            }
        }
    }

    /// Alpha multiplier (0.0..1.0).
    ///
    /// Arrival: fades from 0 to 1.
    /// Departure: fades from 1 to 0.
    pub fn alpha(&self) -> f32 {
        match self.kind {
            TransitionKind::Arriving => {
                let t = (self.elapsed / Self::ARRIVAL_DURATION).clamp(0.0, 1.0);
                ease_out_cubic(t)
            }
            TransitionKind::Departing => {
                let t = (self.elapsed / Self::DEPARTURE_DURATION).clamp(0.0, 1.0);
                (1.0 - t * t).max(0.0)
            }
        }
    }

    /// Whether this transition has completed.
    pub fn is_done(&self) -> bool {
        match self.kind {
            TransitionKind::Arriving => self.elapsed >= Self::ARRIVAL_DURATION,
            TransitionKind::Departing => self.elapsed >= Self::DEPARTURE_DURATION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underdamped_reaches_target() {
        // After sufficient time, should converge near target
        let val = underdamped(5.0, 420.0, 20.0, 9.0, 4.0);
        assert!((val - 420.0).abs() < 0.1, "expected ~420, got {val}");
    }

    #[test]
    fn underdamped_overshoots() {
        // Early in the animation, should overshoot past target
        let val = underdamped(0.15, 420.0, 20.0, 9.0, 4.0);
        assert!(val != 420.0, "should not be exactly at target");
    }

    #[test]
    fn ease_out_cubic_endpoints() {
        assert!((ease_out_cubic(0.0)).abs() < 0.001);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 0.001);
    }

    #[test]
    fn ease_out_cubic_fast_start() {
        // At t=0.5, should be past 0.5 (ease-out is front-loaded)
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn squared_sine_range() {
        for i in 0..100 {
            let t = i as f32 * 0.1;
            let v = squared_sine(t, 1.2);
            assert!((0.0..=1.0).contains(&v), "squared_sine({t}) = {v}");
        }
    }

    #[test]
    fn concave_fade_endpoints() {
        assert!((concave_fade(0.0, 1.0) - 1.0).abs() < 0.001);
        assert!(concave_fade(1.0, 1.0).abs() < 0.001);
    }

    #[test]
    fn overdamped_decays() {
        let v0 = overdamped(0.0, 0.9, 8.0, 2.0);
        let v1 = overdamped(1.0, 0.9, 8.0, 2.0);
        assert!(v0 > v1, "should decay over time");
        assert!(v1 < 0.2, "should be near zero after 1s");
    }

    #[test]
    fn file_entry_scale_starts_large() {
        assert!((file_entry_scale(0.0) - 1.3).abs() < 0.01);
    }

    #[test]
    fn file_entry_scale_settles() {
        assert!((file_entry_scale(2.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn flicker_is_binary() {
        for i in 0..100 {
            let v = flicker_intensity(i as f32 * 0.05, 12345);
            assert!(v == 1.0 || v == 0.65, "expected binary, got {v}");
        }
    }

    #[test]
    fn idle_pulse_zero_at_low_heat() {
        assert_eq!(idle_pulse(0.1, 1.0), 0.0);
        assert_eq!(idle_pulse(0.0, 5.0), 0.0);
    }

    #[test]
    fn idle_pulse_nonzero_at_high_heat() {
        // Over many samples, at least some should be nonzero
        let any_nonzero = (0..100).any(|i| idle_pulse(0.8, i as f32 * 0.1).abs() > 0.001);
        assert!(any_nonzero, "high heat should produce pulse");
    }

    #[test]
    fn reveal_phases_sequence() {
        let mut reveal = PanelReveal::new(420.0);
        reveal.trigger_open();

        // Should start in Crack
        let s = reveal.update();
        assert_eq!(s.phase, RevealPhase::Crack);
        assert!((s.width - 2.0).abs() < 0.1);
        assert_eq!(s.heat_override, Some(1.0));
    }

    #[test]
    fn reveal_completes() {
        let mut reveal = PanelReveal::new(420.0);
        reveal.trigger_open();

        // Fast-forward past total duration
        std::thread::sleep(Duration::from_millis(400));
        let s = reveal.update();
        assert_eq!(s.phase, RevealPhase::Done);
        assert_eq!(s.heat_override, None);
    }

    #[test]
    fn burn_in_starts_at_zero() {
        let m = BurnInMitigation::new();
        assert_eq!(m.offset(), egui::Vec2::ZERO);
    }

    #[test]
    fn burn_in_no_shift_before_interval() {
        let mut m = BurnInMitigation::new();
        let offset = m.update();
        assert_eq!(offset, egui::Vec2::ZERO);
    }

    // ── AgentShake tests ──

    #[test]
    fn shake_inactive_at_low_heat() {
        let mut shake = AgentShake::new();
        let offset = shake.update(0.016, 0.3);
        assert_eq!(offset, 0.0);
        assert!(!shake.is_active());
    }

    #[test]
    fn shake_triggers_on_spike() {
        let mut shake = AgentShake::new();
        // Ramp up slowly (no spike -- delta < SPIKE_DELTA)
        shake.update(0.016, 0.3);
        shake.update(0.016, 0.5);
        // Big spike: delta = 0.4, heat > threshold
        let offset = shake.update(0.016, 0.9);
        assert!(shake.is_active());
        assert!(offset.abs() > 0.0, "offset should be nonzero: {offset}");
    }

    #[test]
    fn shake_decays_to_zero() {
        let mut shake = AgentShake::new();
        shake.update(0.016, 0.3);
        shake.update(0.016, 0.9); // trigger spike
        // Run many frames at constant heat (no new spikes)
        for _ in 0..100 {
            shake.update(0.016, 0.9);
        }
        assert!(!shake.is_active(), "shake should decay after spike");
    }

    #[test]
    fn shake_no_trigger_without_threshold() {
        let mut shake = AgentShake::new();
        // Spike delta is big, but heat is below threshold
        shake.update(0.016, 0.1);
        shake.update(0.016, 0.5); // delta=0.4 but heat=0.5 < 0.6
        assert!(!shake.is_active());
    }

    // ── FileTransition tests ──

    #[test]
    fn arrival_starts_offset_and_transparent() {
        let t = FileTransition::arrival();
        assert!(t.x_offset() < -40.0, "should start far left: {}", t.x_offset());
        assert!(t.alpha() < 0.1, "should start transparent: {}", t.alpha());
    }

    #[test]
    fn arrival_ends_settled() {
        let t = FileTransition::arrival_at(FileTransition::ARRIVAL_DURATION + 0.01);
        assert!(t.x_offset().abs() < 1.0, "should end at x=0: {}", t.x_offset());
        assert!((t.alpha() - 1.0).abs() < 0.01, "should end opaque: {}", t.alpha());
        assert!(t.is_done());
    }

    #[test]
    fn departure_starts_opaque() {
        let t = FileTransition::departure();
        assert!((t.alpha() - 1.0).abs() < 0.01, "should start opaque");
        assert!(t.x_offset().abs() < 0.01, "should start in place");
    }

    #[test]
    fn departure_fades_and_slides() {
        let t = FileTransition::departure_at(FileTransition::DEPARTURE_DURATION);
        assert!(t.alpha() < 0.01, "should end transparent: {}", t.alpha());
        assert!(t.x_offset() > 20.0, "should slide right: {}", t.x_offset());
        assert!(t.is_done());
    }

    #[test]
    fn transition_midpoint_arrival() {
        let t = FileTransition::arrival_at(FileTransition::ARRIVAL_DURATION / 2.0);
        assert!(t.x_offset() > -50.0, "should be partially slid in");
        assert!(t.x_offset() < 0.0, "should not be fully in yet");
        assert!(t.alpha() > 0.0 && t.alpha() < 1.0, "should be partially visible");
    }
}
