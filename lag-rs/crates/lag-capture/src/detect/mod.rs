//! Game detection (port of `Services/VfrCapture/GameDetector.cs`, reworked 2026-11-20 after the owner's CS2
//! replay came out as the ChatGPT window). Which window is "the game" to record:
//! * `window`      reads the facts of every top-level window (process, class, title, true fullscreen, focus);
//! * `library`     knows the games without a list of our own: the Windows games list + launcher folders;
//! * `gpu_history` tells a sustained 3D load (a game) from a spike (an app animating);
//! * `catalog`     names known games and engine windows, and excludes the shell, browsers, chat apps;
//! * `verdict`     turns those facts into "game or not, how sure, how strong" (pure);
//! * `lock_policy` decides when a running lock gives way to a better game or back to the desktop (pure).
mod catalog;
mod gpu_history;
mod library;
mod lock_policy;
mod verdict;
mod window;

pub use lock_policy::{LockChange, LockReview};

use std::collections::HashSet;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetForegroundWindow};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameCandidate {
    pub hwnd: isize,
    pub pid: u32,
    pub exe: String,
    pub game_name: String,
    pub score: i32,
    pub fullscreen: bool,
    /// From a certain source (known list, Windows games list, launcher install in use) vs. a guess.
    pub certain: bool,
}

#[derive(Default)]
pub struct GameDetector {
    gpu: gpu_history::GpuHistory,
    library: library::GameLibrary,
}

impl GameDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every window that qualifies as a game right now (windows other than the recorded monitor's need a
    /// certain source: weak signals are measured against the recorded monitor).
    pub fn candidates(&mut self, monitor_device_id: Option<&str>) -> Vec<GameCandidate> {
        self.gpu.sample();
        self.library.refresh_if_stale();
        let foreground = unsafe { GetForegroundWindow().0 as isize };
        enumerate_windows()
            .into_iter()
            .filter_map(|hwnd| window::WindowFacts::read(hwnd, monitor_device_id, foreground))
            .filter_map(|w| self.judge(&w))
            .collect()
    }

    /// The window to record: certain games first, then the highest score.
    pub fn find_best_game(&mut self, monitor_device_id: Option<&str>) -> Option<GameCandidate> {
        self.candidates(monitor_device_id).into_iter().max_by_key(|c| (c.certain, c.score))
    }

    /// Names normalized for case-insensitive audio-picker membership.
    pub fn running_game_exes(&mut self, monitor_device_id: Option<&str>) -> HashSet<String> {
        self.candidates(monitor_device_id).into_iter().map(|c| c.exe.to_lowercase()).collect()
    }

    /// Whether a running process is a game from a certain source (for a lock whose window does not qualify
    /// right now, e.g. minimized).
    pub fn is_certain_process(&mut self, pid: u32) -> bool {
        self.library.refresh_if_stale();
        window::process_path(pid).is_some_and(|path| {
            let exe = path.rsplit(['\\', '/']).next().unwrap_or("");
            catalog::known_name(exe).is_some() || self.library.is_windows_game(&path)
        })
    }

    fn judge(&self, w: &window::WindowFacts) -> Option<GameCandidate> {
        let verdict = verdict::judge(&verdict::Evidence {
            exe: &w.exe,
            title: &w.title,
            windows_game: self.library.is_windows_game(&w.path),
            launcher_install: self.library.is_launcher_install(&w.path),
            engine_window: catalog::is_engine_class(&w.class),
            fullscreen: w.fullscreen,
            focused: w.focused,
            sustained_gpu: self.gpu.sustained(w.pid),
        })?;
        Some(GameCandidate {
            hwnd: w.hwnd,
            pid: w.pid,
            exe: w.exe.clone(),
            game_name: verdict.name,
            score: verdict.score,
            fullscreen: w.fullscreen,
            certain: verdict.certain,
        })
    }
}

/// Every top-level window, in Z order.
fn enumerate_windows() -> Vec<isize> {
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        // EnumWindows is synchronous: the Vec outlives every callback.
        let list = unsafe { &mut *(param.0 as *mut Vec<isize>) };
        list.push(hwnd.0 as isize);
        BOOL(1)
    }
    let mut list = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_this_machine_headlessly() {
        let mut d = GameDetector::new();
        let _ = d.find_best_game(None);
        let _ = d.is_certain_process(std::process::id());
    }
    /// What the detector sees on this PC right now (read only):
    /// `cargo test -p lag-capture --release -- --ignored --nocapture print_live_candidates`
    #[test]
    #[ignore]
    fn print_live_candidates() {
        let mut d = GameDetector::new();
        for _ in 0..3 {
            let _ = d.candidates(None); // fill the GPU history (sustained load needs 3 samples)
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        let mut all = d.candidates(None);
        all.sort_by_key(|c| std::cmp::Reverse((c.certain, c.score)));
        println!("best: {:?}", all.first().map(|c| (&c.exe, c.certain, c.score)));
        for c in &all {
            println!(
                "  {} \"{}\" certain={} score={} fullscreen={}",
                c.exe, c.game_name, c.certain, c.score, c.fullscreen
            );
        }
    }
}
