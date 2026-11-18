//! Headless live test of the Rust replay engine on a running game (no window is opened).
//!
//! Usage: `engine_probe <exe-name> <seconds> <out-dir>` e.g. `engine_probe cs2.exe 30 E:\tmp\probe`.
//! Reads fps/bitrate/height/codec from the user's settings.json, force-locks the game's main window,
//! records `seconds`, saves one clip into `out-dir` and prints the clip path + engine status.
use lag_capture::engine::{EngineEvent, EngineOptions, ReplayEngine};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
};

struct StderrLog(Instant);
impl log::Log for StderrLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        eprintln!("[{:8.3}] {}", self.0.elapsed().as_secs_f64(), r.args());
    }
    fn flush(&self) {}
}

fn pid_of(exe: &str) -> Option<u32> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {exe}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().find_map(|l| l.split("\",\"").nth(1).and_then(|p| p.trim_matches('"').parse().ok()))
}

/// Largest visible unowned top-level window of `pid`.
fn main_window(pid: u32) -> Option<isize> {
    struct Ctx {
        pid: u32,
        best: Option<(isize, i64)>,
    }
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(l.0 as *mut Ctx) };
        let mut p = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut p)) };
        let owned = unsafe { GetWindow(h, GW_OWNER) }.map(|o| !o.0.is_null()).unwrap_or(false);
        if p == ctx.pid && unsafe { IsWindowVisible(h) }.as_bool() && !owned {
            let mut r = RECT::default();
            if unsafe { GetWindowRect(h, &mut r) }.is_ok() {
                let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
                if ctx.best.is_none_or(|b| area > b.1) {
                    ctx.best = Some((h.0 as isize, area));
                }
            }
        }
        true.into()
    }
    let mut ctx = Ctx { pid, best: None };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.best.map(|b| b.0)
}

fn main() {
    let t0 = Instant::now();
    log::set_boxed_logger(Box::new(StderrLog(t0))).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    let args: Vec<String> = std::env::args().collect();
    let exe = args.get(1).map(String::as_str).unwrap_or("cs2.exe");
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let out_dir =
        args.get(3).cloned().unwrap_or_else(|| std::env::temp_dir().join("lag-probe").to_string_lossy().into());

    let pid = pid_of(exe).unwrap_or_else(|| panic!("{exe} is not running"));
    let hwnd = main_window(pid).unwrap_or_else(|| panic!("no window for {exe} (pid {pid})"));
    let s = lag_core::settings::default_path()
        .and_then(|p| lag_core::settings::SettingsData::load(&p).ok().flatten())
        .unwrap_or_default();
    let opts = EngineOptions {
        library_path: out_dir,
        buffer_seconds: secs as f64 + 10.0,
        preferred_codec: s.codec_name.clone().filter(|c| !c.is_empty()),
        bitrate_kbps: s.bitrate_kbps,
        target_height: s.output_resolution_height,
        max_fps: s.frame_rate,
        forced_window: hwnd,
        audio_enabled: std::env::var_os("LAG_NOAUDIO").is_none(),
        separate_audio_tracks: s.separate_audio_tracks,
        apps_audio_mode: s.audio_capture_mode.as_deref() == Some("apps"),
        audio_mode_by_target: s.audio_mode_by_target,
        audio_apps: s
            .audio_apps
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.enabled)
            .map(|a| (a.exe.unwrap_or_default(), a.volume as f32 / 100.0))
            .collect(),
        // C# VfrRecorderAdapter mapping: mic disabled -> "" (none); empty id -> default mic.
        microphone_id: if !s.mic_enabled {
            Some(String::new())
        } else {
            s.microphone_id.clone().filter(|m| !m.is_empty())
        },
        mic_volume: s.mic_volume as f32 / 100.0,
        mic_mono: s.mic_mono,
        system_audio_enabled: s.system_audio_enabled,
        system_volume: s.system_audio_volume as f32 / 100.0,
        game_audio_enabled: s.game_audio_enabled,
        game_volume: s.game_audio_volume as f32 / 100.0,
        noise_suppression: s.noise_suppression,
        input_gate_auto: s.input_sensitivity_auto,
        input_gate_threshold: s.input_sensitivity_threshold as f32 / 100.0,
        follow_focus: std::env::var_os("LAG_FOLLOW").is_some(),
        ..Default::default()
    };
    eprintln!(
        "probe: {exe} pid {pid} hwnd {hwnd:#x}; fps cap {}, bitrate {}, height {}, {secs}s",
        opts.max_fps, opts.bitrate_kbps, opts.target_height
    );
    let engine = ReplayEngine::new(Arc::new(|e: EngineEvent| eprintln!("event: {e:?}")));
    engine.start(opts).expect("engine start");
    std::thread::sleep(Duration::from_secs(secs + 1)); // +1: the lock timer first fires at 0.5 s
    let st = engine.status();
    eprintln!("status: {st:?}");
    // LAG_STOPONLY: the app-exit path (RecorderService::shutdown_blocking): stop while recording, no save.
    if std::env::var_os("LAG_STOPONLY").is_some() {
        let t = Instant::now();
        engine.stop();
        eprintln!("STOP while recording took {:?}", t.elapsed());
        drop(engine);
        eprintln!("probe exiting normally");
        return;
    }
    let t = Instant::now();
    let clip = engine.save_replay();
    eprintln!("save took {:?}", t.elapsed());
    engine.stop();
    match clip {
        Some(p) => println!("{p}"),
        None => {
            eprintln!("no clip saved");
            std::process::exit(1);
        }
    }
}
