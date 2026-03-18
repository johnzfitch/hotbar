//! Fire automaton constants and hot-spot scanning.
//!
//! The fire automaton and rendering are now part of `ChromeHeatPass`
//! in `chrome.rs`. This module retains shared constants and the
//! `scan_hot_spots` function used by both the pass and tests.

/// Maximum fire column height (entries, one per pixel row).
/// 4096 supports up to 4K vertical monitors.
pub const MAX_FIRE_HEIGHT: usize = 4096;

/// Scan a fire column slice for positions exceeding `threshold`.
///
/// Skips 8 pixels after each hit to avoid clustered spawns.
/// Public so `ChromeHeatPass` can reuse this logic.
pub fn scan_hot_spots(column: &[f32], threshold: f32, height: u32) -> Vec<f32> {
    let h = (height as usize).min(column.len());
    let mut spots = Vec::new();
    let mut y = 0;
    while y < h {
        if column[y] > threshold {
            spots.push(y as f32);
            y += 8;
        } else {
            y += 1;
        }
    }
    spots
}

#[cfg(test)]
mod tests {
    #[test]
    fn fire_automaton_propagates_upward() {
        let h = 100;
        let mut col = vec![0.0f32; h];
        let mut rng: u32 = 42;

        let mut next_f32 = || -> f32 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng as f32) / (u32::MAX as f32)
        };

        for _ in 0..50 {
            col[h - 1] = 0.8 + next_f32() * 0.2;
            col[h - 2] = 0.5 + next_f32() * 0.5;

            for y in (0..h - 2).rev() {
                let n3 = if y + 3 < h { col[y + 3] } else { 0.0 };
                let sum = col[y + 1] + col[y] + col[y + 2] + n3;
                col[y] = (sum * 0.25 - 0.012).max(0.0);
            }
        }

        assert!(col[h - 1] > 0.5, "bottom should be hot: {}", col[h - 1]);
        assert!(col[h / 2] > 0.0, "middle should have heat: {}", col[h / 2]);
        assert!(
            col[0] < col[h - 1],
            "top should be cooler: top={} bottom={}",
            col[0],
            col[h - 1]
        );
    }

    #[test]
    fn fire_automaton_cools_down() {
        let h = 50;
        let mut col = vec![0.5f32; h];

        for _ in 0..100 {
            for val in col.iter_mut() {
                *val = (*val - 0.02).max(0.0);
            }
        }

        assert!(col.iter().all(|&v| v == 0.0), "should cool to zero");
    }

    #[test]
    fn fire_palette_transparent_at_zero() {
        let heat = 0.0;
        let h = (heat + 0.0_f32 * 0.08).fract();
        assert!(h < 0.15, "zero heat should be in transparent band");
    }

    #[test]
    fn fire_palette_cycling_shifts_lookup() {
        let heat = 0.5;
        let h0 = (heat + 0.0 * 0.08_f32).fract();
        let h1 = (heat + 10.0 * 0.08_f32).fract();
        assert_ne!(h0, h1, "palette should shift with time");
    }

    #[test]
    fn hot_spots_empty_for_cold_column() {
        let col = vec![0.0f32; 100];
        let spots = super::scan_hot_spots(&col, 0.7, 100);
        assert!(spots.is_empty(), "cold column should have no hot spots");
    }

    #[test]
    fn hot_spots_returns_positions_for_hot_column() {
        let mut col = vec![0.0f32; 100];
        for item in col.iter_mut().take(60).skip(50) {
            *item = 0.9;
        }
        let spots = super::scan_hot_spots(&col, 0.7, 100);
        assert!(!spots.is_empty(), "should find hot spots");
        assert!(spots[0] >= 50.0 && spots[0] < 60.0);
    }

    #[test]
    fn hot_spots_skips_8px_after_hit() {
        let mut col = vec![0.0f32; 100];
        for item in col.iter_mut().take(36).skip(20) {
            *item = 0.9;
        }
        let spots = super::scan_hot_spots(&col, 0.7, 100);
        assert!(spots.len() <= 2, "should skip 8px: got {} spots", spots.len());
    }
}
