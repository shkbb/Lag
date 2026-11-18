//! Mirrors Services/VfrCapture/NoiseGate.cs, including Reset's retained UI state.
#[derive(Debug, Clone)]
pub struct NoiseGate {
    pub auto: bool,
    pub manual_threshold: f32,
    pub threshold: f32,
    pub is_open: bool,
    noise_floor: f32,
    gain: f32,
    hold_remaining: f32,
}
impl Default for NoiseGate {
    fn default() -> Self {
        Self {
            auto: true,
            manual_threshold: 0.08,
            threshold: 0.,
            is_open: false,
            noise_floor: 0.02,
            gain: 0.,
            hold_remaining: 0.,
        }
    }
}
impl NoiseGate {
    pub fn process(&mut self, level: f32, dt: f64) -> f32 {
        let t = dt.clamp(0.0001, 0.5) as f32;
        if self.auto {
            let rate = if level < self.noise_floor { 2.5 } else { 0.03 };
            self.noise_floor += (level - self.noise_floor) * (rate * t).min(1.);
            self.noise_floor = self.noise_floor.clamp(0.004, 0.5);
            self.threshold = (self.noise_floor * 2.3).clamp(0.012, 0.9);
        } else {
            self.threshold = self.manual_threshold.clamp(0., 1.);
        }
        let above = level >= self.threshold;
        self.hold_remaining = if above { 0.28 } else { (self.hold_remaining - t).max(0.) };
        self.is_open = above || self.hold_remaining > 0.;
        let target = if self.is_open { 1. } else { 0. };
        self.gain += (target - self.gain) * ((if target > self.gain { 70. } else { 7. }) * t).min(1.);
        if self.gain < 0.0005 {
            self.gain = 0.;
        }
        self.gain
    }
    pub fn reset(&mut self) {
        self.gain = 0.;
        self.hold_remaining = 0.;
        self.noise_floor = 0.02;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attack_hold_release_and_reset() {
        let mut g = NoiseGate { auto: false, ..Default::default() };
        assert!((g.process(0.1, 0.01) - 0.7).abs() < 1e-6);
        assert!((g.process(0.1, 0.01) - 0.91).abs() < 1e-6);
        g.process(0., 0.27);
        assert!(g.is_open);
        let gain = g.process(0., 0.02);
        assert!(!g.is_open);
        assert!((gain - 0.86).abs() < 1e-5);
        g.reset();
        assert_eq!(g.threshold, 0.08);
        assert!(!g.is_open);
        assert!((g.process(1., 0.) - 0.007).abs() < 1e-6);
    }
    #[test]
    fn auto_floor_and_clamps() {
        let mut g = NoiseGate::default();
        g.process(0., 0.1);
        assert!((g.threshold - 0.0345).abs() < 1e-6);
        for _ in 0..100 {
            g.process(0., 1.);
        }
        assert_eq!(g.threshold, 0.012);
        for _ in 0..1000 {
            g.process(1., 0.5);
        }
        assert_eq!(g.threshold, 0.9);
        g.auto = false;
        g.manual_threshold = 2.;
        g.process(1., 0.1);
        assert_eq!(g.threshold, 1.);
    }
}
