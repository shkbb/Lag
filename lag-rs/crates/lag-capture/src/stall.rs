//! Port of `CaptureStallMonitor.cs`: attributes every frame gap longer than 25 ms either to the SOURCE
//! (the game/DWM presented nothing) or to OURS (our capture thread was busy), and reports `[stall]` lines.
//!
//! Rust has no garbage collector, so the GC columns are always 0 and the "gc" cause never occurs; the log
//! format is kept byte-for-byte so existing log-analysis tools keep working.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

pub const DEFAULT_THRESHOLD_US: i64 = 25_000;
const RECENT_FRAMES: usize = 8;
const MAX_QUEUED_EVENTS: usize = 64;
const MAX_REPORTED_EVENTS: usize = 6;

#[derive(Debug, Default, Clone, Copy)]
pub struct Work {
    pub total_us: i64,
    pub overlay_us: i64,
    pub convert_us: i64,
    pub encode_us: i64,
    pub retune_us: i64,
    pub gc_us: i64,
    pub idle_gc_us: i64,
}

impl Work {
    pub fn other_us(&self) -> i64 {
        (self.total_us - self.overlay_us - self.convert_us - self.encode_us - self.retune_us).max(0)
    }
    pub fn blocking_us(&self) -> i64 {
        self.total_us + self.idle_gc_us
    }
}

#[derive(Debug, Clone)]
struct StallEvent {
    gap_us: i64,
    when: String,
    ours: bool,
    cause: &'static str,
    worst: Work,
}

pub struct CaptureStallMonitor {
    threshold_us: i64,
    // capture thread only
    recent: [Work; RECENT_FRAMES],
    recent_next: usize,
    prev_present_us: Option<i64>,
    cur_start: Option<Instant>,
    // shared with the reporting thread
    events: Mutex<VecDeque<StallEvent>>,
    source_stalls: AtomicI64,
    our_stalls: AtomicI64,
}

impl Default for CaptureStallMonitor {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD_US)
    }
}

impl CaptureStallMonitor {
    pub fn new(threshold_us: i64) -> Self {
        Self {
            threshold_us,
            recent: [Work::default(); RECENT_FRAMES],
            recent_next: 0,
            prev_present_us: None,
            cur_start: None,
            events: Mutex::new(VecDeque::new()),
            source_stalls: AtomicI64::new(0),
            our_stalls: AtomicI64::new(0),
        }
    }

    pub fn reset(&mut self) {
        self.recent = [Work::default(); RECENT_FRAMES];
        self.recent_next = 0;
        self.prev_present_us = None;
    }

    pub fn frame_arrived(&mut self, present_us: i64, _arrival_us: i64) {
        self.cur_start = Some(Instant::now());
        if let Some(prev) = self.prev_present_us {
            let gap = present_us - prev;
            if gap > self.threshold_us {
                self.classify(gap);
            }
        }
        self.prev_present_us = Some(present_us);
    }

    pub fn frame_done(&mut self, overlay_us: i64, convert_us: i64, encode_us: i64, retune_us: i64) {
        let total_us = self.cur_start.map_or(0, |s| s.elapsed().as_micros() as i64);
        self.recent[self.recent_next] =
            Work { total_us, overlay_us, convert_us, encode_us, retune_us, gc_us: 0, idle_gc_us: 0 };
        self.recent_next = (self.recent_next + 1) % RECENT_FRAMES;
    }

    fn classify(&mut self, gap_us: i64) {
        let worst = self.recent.iter().copied().max_by_key(Work::blocking_us).unwrap_or_default();
        let ours = worst.blocking_us() * 2 >= gap_us;
        let cause = if ours { dominant(&worst) } else { "source" };
        if ours {
            self.our_stalls.fetch_add(1, Ordering::Relaxed);
        } else {
            self.source_stalls.fetch_add(1, Ordering::Relaxed);
        }
        let mut q = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() < MAX_QUEUED_EVENTS {
            q.push_back(StallEvent { gap_us, when: local_time_hms_ms(), ours, cause, worst });
        }
    }

    /// The `[stall]` report for the window since the last call, or `None` when there were no stalls.
    pub fn drain_report(&self) -> Option<String> {
        let src = self.source_stalls.swap(0, Ordering::Relaxed);
        let ours = self.our_stalls.swap(0, Ordering::Relaxed);
        let mut q = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if src + ours == 0 {
            q.clear();
            return None;
        }
        let mut s = format!(
            "[stall] {} gap(s) >{}ms: {} SOURCE (game/DWM presented nothing), {} OURS",
            src + ours,
            self.threshold_us / 1000,
            src,
            ours
        );
        let (mut shown, mut more) = (0, 0);
        while let Some(e) = q.pop_front() {
            if shown >= MAX_REPORTED_EVENTS {
                more += 1;
                continue;
            }
            shown += 1;
            let w = e.worst;
            let ms = |us: i64| us as f64 / 1000.0;
            let _ = write!(s, "\n    {:6.1}ms gap at {} -> ", ms(e.gap_us), e.when);
            if e.ours {
                let _ = write!(
                    s,
                    "OURS: {} | frame {:.1}ms = overlay {:.1} + convert {:.1} + encode {:.1} + retune {:.1} + other {:.1} | gc in-frame {:.1} idle {:.1}",
                    e.cause,
                    ms(w.total_us),
                    ms(w.overlay_us),
                    ms(w.convert_us),
                    ms(w.encode_us),
                    ms(w.retune_us),
                    ms(w.other_us()),
                    ms(w.gc_us),
                    ms(w.idle_gc_us)
                );
            } else {
                let _ = write!(s, "SOURCE (heaviest recent frame of ours: {:.1}ms)", ms(w.blocking_us()));
            }
        }
        if more > 0 {
            let _ = write!(s, "\n    ... and {more} more");
        }
        Some(s)
    }
}

fn dominant(w: &Work) -> &'static str {
    if (w.gc_us + w.idle_gc_us) * 2 >= w.blocking_us() && w.blocking_us() > 0 && w.gc_us + w.idle_gc_us > 0 {
        return "gc";
    }
    let mut best = ("other", w.other_us());
    for (name, us) in
        [("overlay", w.overlay_us), ("convert", w.convert_us), ("encode", w.encode_us), ("retune", w.retune_us)]
    {
        if us > best.1 {
            best = (name, us);
        }
    }
    best.0
}

/// Local wall-clock time as `HH:mm:ss.fff`.
pub fn local_time_hms_ms() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}.{:03}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_vs_ours_and_report_format() {
        let mut m = CaptureStallMonitor::default();
        assert!(m.drain_report().is_none());
        m.frame_arrived(0, 0);
        m.frame_done(0, 100, 500, 0);
        m.frame_arrived(40_000, 40_000); // 40 ms gap, our work tiny -> SOURCE
        m.frame_done(0, 100, 500, 0);
        m.recent[1] = Work { total_us: 30_000, encode_us: 28_000, ..Work::default() };
        m.frame_arrived(90_000, 90_000); // 50 ms gap, our heaviest 30 ms*2 >= 50 -> OURS: encode
        let r = m.drain_report().unwrap();
        assert!(r.starts_with("[stall] 2 gap(s) >25ms: 1 SOURCE (game/DWM presented nothing), 1 OURS"), "{r}");
        assert!(r.contains("  40.0ms gap at "), "{r}");
        assert!(r.contains("-> SOURCE (heaviest recent frame of ours: "), "{r}");
        assert!(r.contains("-> OURS: encode | frame 30.0ms = overlay 0.0 + convert 0.0 + encode 28.0 + retune 0.0 + other 2.0 | gc in-frame 0.0 idle 0.0"), "{r}");
        assert!(m.drain_report().is_none());
    }
}
