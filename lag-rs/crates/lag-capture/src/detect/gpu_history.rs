//! Sustained GPU load per process. A game keeps the 3D engine busy all the time; an app (ChatGPT, Discord, a
//! browser) spikes it for a moment while something animates. One sample above the threshold made such an app
//! look like a game, so a load counts only when it held over most of the last few samples (one every ~2 s).
use crate::overlay::gpu_usage::GpuUsageProbe;
use std::collections::{HashMap, VecDeque};

/// 3D engine utilisation (%) a game keeps up.
const THRESHOLD: f64 = 30.0;
/// Samples the average is taken over (the detector samples about every 2 s).
const WINDOW: usize = 3;

/// The last few loads of one process.
#[derive(Default, Clone)]
pub(super) struct LoadWindow(VecDeque<f64>);

impl LoadWindow {
    pub(super) fn push(&mut self, load: f64) {
        if self.0.len() == WINDOW {
            self.0.pop_front();
        }
        self.0.push_back(load);
    }
    /// A full window whose MEDIAN reaches the threshold: one spike cannot carry it (an average could:
    /// 90 + 5 + 4 averages 33 %), while a game's load that wobbles around the threshold still does.
    pub(super) fn sustained(&self) -> bool {
        if self.0.len() < WINDOW {
            return false;
        }
        let mut sorted: Vec<f64> = self.0.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        sorted[WINDOW / 2] >= THRESHOLD
    }
}

#[derive(Default)]
pub(super) struct GpuHistory {
    probe: GpuUsageProbe,
    loads: HashMap<u32, LoadWindow>,
}

impl GpuHistory {
    /// One sample for every process: a process the counter no longer lists counts as idle, and a process
    /// idle for a whole window is forgotten.
    pub(super) fn sample(&mut self) {
        self.probe.sample();
        let current = self.probe.loads();
        for (pid, window) in self.loads.iter_mut() {
            window.push(current.get(pid).copied().unwrap_or(0.0));
        }
        for (&pid, &load) in current {
            self.loads.entry(pid).or_insert_with(|| {
                let mut w = LoadWindow::default();
                w.push(load);
                w
            });
        }
        self.loads.retain(|_, w| w.0.iter().any(|l| *l > 0.0));
    }
    pub(super) fn sustained(&self, pid: u32) -> bool {
        self.loads.get(&pid).is_some_and(LoadWindow::sustained)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_spike_is_not_a_game() {
        let mut w = LoadWindow::default();
        w.push(90.0);
        assert!(!w.sustained(), "one sample is not enough");
        w.push(5.0);
        w.push(4.0);
        assert!(!w.sustained(), "an animation spike averages out");
        let mut g = LoadWindow::default();
        for load in [35.0, 28.0, 40.0] {
            g.push(load);
        }
        assert!(g.sustained(), "a game holds the load");
        g.push(0.0);
        g.push(0.0);
        assert!(!g.sustained(), "and loses it when it stops");
    }
}
