//! Port of `CadenceGrid.cs`: the "VFR stabiliser" that snaps frame timestamps onto a 1/fps grid.
//!
//! Each frame goes to the NEAREST slot (never moved more than half a step); a frame landing in an
//! already-filled slot is dropped (this also enforces the fps cap); an empty slot leaves a double-length
//! interval, exactly like a genuinely missed frame. Slot positions are derived from `fps` (not from a
//! rounded step) so rates like 165 fps (6060.6 us) never drift against the audio clock.
//!
//! Driven by the single capture thread; only [`CadenceGrid::take_collisions`] may be called from another
//! thread (the stats logger), hence the atomic counter.

use std::sync::atomic::{AtomicI64, Ordering};

pub struct CadenceGrid {
    fps: i64,
    origin_us: i64,
    pinned: bool,
    last_slot: Option<i64>,
    collisions: AtomicI64,
}

impl CadenceGrid {
    /// `fps` = slots per second (the frame-rate cap); `<= 0` disables snapping.
    pub fn new(fps: i64) -> Self {
        Self { fps, origin_us: 0, pinned: false, last_slot: None, collisions: AtomicI64::new(0) }
    }

    pub fn enabled(&self) -> bool {
        self.fps > 0
    }

    pub fn fps(&self) -> f64 {
        self.fps as f64
    }

    /// Nominal slot spacing in us (reporting only).
    pub fn step_us(&self) -> i64 {
        if self.fps > 0 {
            1_000_000 / self.fps
        } else {
            0
        }
    }

    /// Re-anchor on the next frame (new capture session, resume after pause).
    pub fn repin(&mut self) {
        self.pinned = false;
        self.last_slot = None;
    }

    /// Reads and clears the collision counter.
    pub fn take_collisions(&self) -> i64 {
        self.collisions.swap(0, Ordering::Relaxed)
    }

    /// `pts_us` snapped to the grid, or `None` when its slot is already taken (C# returned `Drop = -1`).
    pub fn snap(&mut self, pts_us: i64) -> Option<i64> {
        if self.fps <= 0 {
            return Some(pts_us);
        }
        if !self.pinned {
            self.origin_us = pts_us;
            self.pinned = true;
            self.last_slot = None;
        }
        let delta = pts_us - self.origin_us;
        // Round half away from zero, identical to the C# integer maths.
        let slot = if delta >= 0 {
            (delta * self.fps + 500_000) / 1_000_000
        } else {
            (delta * self.fps - 500_000) / 1_000_000
        };
        if let Some(last) = self.last_slot {
            if slot <= last {
                self.collisions.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        }
        self.last_slot = Some(slot);
        // div_euclid: C# truncated toward zero for negative slots (a frame before the origin landed up
        // to one step late). Negative slots only happen if the clock steps backwards, which the engine
        // clock never does; for every real (non-negative) slot the result is identical to C#.
        Some(self.origin_us + (slot * 1_000_000).div_euclid(self.fps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_grid_passes_through() {
        let mut g = CadenceGrid::new(0);
        assert_eq!(g.snap(12345), Some(12345));
        assert!(!g.enabled());
    }

    #[test]
    fn snaps_to_nearest_slot_and_drops_collisions() {
        let mut g = CadenceGrid::new(100); // 10 ms grid
        assert_eq!(g.snap(1_000_000), Some(1_000_000)); // pins origin
        assert_eq!(g.snap(1_010_400), Some(1_010_000)); // nearest slot 1
        assert_eq!(g.snap(1_012_000), None); // still slot 1 -> dropped
        assert_eq!(g.snap(1_034_999), Some(1_030_000)); // slot 3 (slot 2 empty = missed frame)
        assert_eq!(g.take_collisions(), 1);
        assert_eq!(g.take_collisions(), 0);
    }

    #[test]
    fn no_drift_at_165fps_over_an_hour() {
        let mut g = CadenceGrid::new(165);
        let start = 5_000_000;
        let mut last = 0;
        let mut origin = None;
        let n: i64 = 165 * 3600;
        for i in 0..n {
            // Ideal 165 fps arrival with +/-1.5 ms jitter.
            let ideal = start + (i * 1_000_000) / 165;
            let jitter = (i * 7919) % 3000 - 1500;
            if let Some(p) = g.snap(ideal + jitter) {
                origin.get_or_insert(p);
                last = p;
            }
        }
        // The grid is anchored on the first frame (incl. its jitter); measure drift from there.
        let expected_last = origin.unwrap() + ((n - 1) * 1_000_000) / 165;
        assert!((last - expected_last).abs() <= 1, "drift {} us", last - expected_last);
    }

    #[test]
    fn repin_reanchors() {
        let mut g = CadenceGrid::new(100);
        g.snap(0);
        g.repin();
        assert_eq!(g.snap(7_777), Some(7_777));
    }
}
