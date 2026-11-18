//! Mirrors VfrReplayEngine.cs EngineNowUs: QPC session origin minus paused duration.
use std::sync::RwLock;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
pub fn qpc() -> i64 {
    let mut v = 0;
    unsafe {
        QueryPerformanceCounter(&mut v).expect("QPC");
    }
    v
}
pub fn qpc_frequency() -> i64 {
    static F: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0;
        unsafe {
            QueryPerformanceFrequency(&mut f).expect("QPC frequency");
        }
        f
    })
}
#[derive(Default, Clone, Copy)]
struct Pause {
    paused: bool,
    start: i64,
    accumulated_us: i64,
}
pub struct EngineClock {
    origin: i64,
    pause: RwLock<Pause>,
}
impl Default for EngineClock {
    fn default() -> Self {
        Self::new()
    }
}
impl EngineClock {
    pub fn new() -> Self {
        Self::from_qpc_origin(qpc())
    }
    pub fn from_qpc_origin(origin: i64) -> Self {
        Self { origin, pause: RwLock::new(Pause::default()) }
    }
    pub fn engine_now_us(&self) -> i64 {
        let p = *self.pause.read().unwrap();
        let ts = qpc();
        (ts - self.origin) * 1_000_000 / qpc_frequency()
            - p.accumulated_us
            - if p.paused { (ts - p.start) * 1_000_000 / qpc_frequency() } else { 0 }
    }
    pub fn is_paused(&self) -> bool {
        self.pause.read().unwrap().paused
    }
    pub fn set_paused(&self, paused: bool) {
        let mut p = self.pause.write().unwrap();
        if p.paused == paused {
            return;
        }
        if paused {
            p.start = qpc();
        } else {
            p.accumulated_us += (qpc() - p.start) * 1_000_000 / qpc_frequency();
        }
        p.paused = paused;
    }
}
/// Explicit session clock argument prevents accidental absolute-QPC/session-epoch mixing.
pub fn engine_now_us(clock: &EngineClock) -> i64 {
    clock.engine_now_us()
}
