//! Port of `Services/VfrCapture/SystemStatsSampler.cs` (one-second CPU/GPU/RAM/FPS snapshots).
use super::gpu_usage::GpuUsageProbe;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
#[derive(Debug, Clone, Copy, Default)]
pub struct Snapshot {
    pub fps: i32,
    pub cpu_percent: i32,
    pub gpu_percent: i32,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
}
// All fields are atomic, so even a reader retry cannot race with a non-atomic write.
// One sampler writes, capture threads read. The version makes the entire snapshot coherent.
#[derive(Default)]
struct Published {
    version: AtomicU64,
    fps: AtomicI32,
    cpu: AtomicI32,
    gpu: AtomicI32,
    used: AtomicU64,
    total: AtomicU64,
}
impl Published {
    fn store(&self, s: Snapshot) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.fps.store(s.fps, Ordering::SeqCst);
        self.cpu.store(s.cpu_percent, Ordering::SeqCst);
        self.gpu.store(s.gpu_percent, Ordering::SeqCst);
        self.used.store(s.ram_used_gb.to_bits(), Ordering::SeqCst);
        self.total.store(s.ram_total_gb.to_bits(), Ordering::SeqCst);
        self.version.fetch_add(1, Ordering::SeqCst);
    }
    fn load(&self) -> Snapshot {
        loop {
            let version = self.version.load(Ordering::SeqCst);
            if version & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let s = Snapshot {
                fps: self.fps.load(Ordering::SeqCst),
                cpu_percent: self.cpu.load(Ordering::SeqCst),
                gpu_percent: self.gpu.load(Ordering::SeqCst),
                ram_used_gb: f64::from_bits(self.used.load(Ordering::SeqCst)),
                ram_total_gb: f64::from_bits(self.total.load(Ordering::SeqCst)),
            };
            if self.version.load(Ordering::SeqCst) == version {
                return s;
            }
        }
    }
}
pub struct SystemStatsSampler {
    frames: Arc<AtomicU64>,
    current: Arc<Published>,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}
fn times() -> Option<(u64, u64, u64)> {
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).ok()?;
    }
    let value = |t: FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
    Some((value(i), value(k), value(u)))
}
fn cpu_percent(previous: (u64, u64, u64), current: (u64, u64, u64)) -> i32 {
    let idle = current.0.saturating_sub(previous.0);
    let total = current.1.saturating_sub(previous.1) + current.2.saturating_sub(previous.2);
    if total == 0 {
        0
    } else {
        (100.0 * (total as f64 - idle as f64) / total as f64).clamp(0.0, 100.0) as i32
    }
}
impl SystemStatsSampler {
    pub fn new() -> windows::core::Result<Self> {
        let frames = Arc::new(AtomicU64::new(0));
        let current = Arc::new(Published::default());
        let (stop, rx) = mpsc::channel();
        let (f, c) = (frames.clone(), current.clone());
        let thread = std::thread::Builder::new()
            .name("overlay stats".into())
            .spawn(move || {
                let mut gpu = GpuUsageProbe::new();
                gpu.sample();
                let mut previous = times().unwrap_or_default();
                let mut deadline = std::time::Instant::now() + Duration::from_secs(1);
                loop {
                    match rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    deadline += Duration::from_secs(1);
                    let mut s = Snapshot { fps: f.swap(0, Ordering::Relaxed) as i32, ..Snapshot::default() };
                    if let Some(now) = times() {
                        s.cpu_percent = cpu_percent(previous, now);
                        previous = now;
                    }
                    s.gpu_percent = gpu.collect().map(|s| s.total.clamp(0.0, 100.0) as i32).unwrap_or(0);
                    let mut mem =
                        MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
                    if unsafe { GlobalMemoryStatusEx(&mut mem) }.is_ok() {
                        s.ram_total_gb = mem.ullTotalPhys as f64 / 1073741824.0;
                        s.ram_used_gb = (mem.ullTotalPhys - mem.ullAvailPhys) as f64 / 1073741824.0;
                    }
                    c.store(s);
                }
            })
            .map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))?;
        Ok(Self { frames, current, stop, thread: Some(thread) })
    }
    pub fn count_frame(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
    pub fn current(&self) -> Snapshot {
        self.current.load()
    }
}
impl Drop for SystemStatsSampler {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_math_includes_kernel_idle() {
        assert_eq!(cpu_percent((100, 200, 100), (125, 250, 150)), 75);
        assert_eq!(cpu_percent((0, 0, 0), (200, 100, 0)), 0);
        assert_eq!(cpu_percent((0, 0, 0), (0, 0, 0)), 0);
    }
    #[test]
    fn published_snapshots_are_coherent_during_updates() {
        let published = Arc::new(Published::default());
        let writer = published.clone();
        let t = std::thread::spawn(move || {
            for n in 1..10_000 {
                writer.store(Snapshot {
                    fps: n,
                    cpu_percent: -n,
                    gpu_percent: n * 2,
                    ram_used_gb: n as f64,
                    ram_total_gb: n as f64 * 2.,
                });
            }
        });
        while !t.is_finished() {
            let s = published.load();
            assert_eq!(s.cpu_percent, -s.fps);
            assert_eq!(s.gpu_percent, s.fps * 2);
            assert_eq!(s.ram_total_gb, s.ram_used_gb * 2.);
            assert_eq!(s.ram_used_gb, s.fps as f64);
        }
        t.join().unwrap();
    }
    #[test]
    fn sampler_stops_before_closing_query() {
        let s = SystemStatsSampler::new().unwrap();
        s.count_frame();
        assert_eq!(s.current().fps, 0);
    }
}
