//! Player v2 smoke probe WITHOUT anything on screen: a never-shown, non-activating top-level window hosts
//! the real NativeVideoWindow child; mpv renders into it with the real Player v2 options except
//! `ao=null` (silent). Nothing is shown, nothing takes focus, no sound: safe while the owner plays.
//!
//! Usage: native_probe <seconds-per-clip> <clip1.mp4> [clip2.mp4 ...]
//! Reports per clip: codec, size, fps, hwdec actually used, drops; then a rapid clip-switch test.
use lag_player::{native_mpv_options, MpvPlayer, NativeVideoWindow, PlayerEvent};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn host_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefWindowProcW(h, m, w, l) }
}

/// Pump this thread's messages (mpv's child window is attached to our input queue) for `d`.
fn pump(d: Duration) {
    let until = Instant::now() + d;
    while Instant::now() < until {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(rx: &mpsc::Receiver<PlayerEvent>, timeout: Duration, pred: impl Fn(&PlayerEvent) -> bool) -> bool {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        pump(Duration::from_millis(10));
        while let Ok(e) = rx.try_recv() {
            if let PlayerEvent::Error(err) = &e {
                println!("  mpv error: {err}");
            }
            if pred(&e) {
                return true;
            }
        }
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    let clips: Vec<String> = args.iter().skip(2).cloned().collect();
    if clips.is_empty() {
        eprintln!("usage: native_probe <seconds> <clip...>");
        std::process::exit(2);
    }
    // LAG_PROBE_SIZE=3440x1440: surface size (fullscreen on the owner's monitor)
    let (pw, ph): (i32, i32) = std::env::var("LAG_PROBE_SIZE")
        .ok()
        .and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
        .unwrap_or((1920, 1080));
    unsafe {
        let hinst = GetModuleHandleW(None).expect("module");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("LagProbeHiddenHost"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
    // NOT WS_VISIBLE and never shown: no pixels on screen, no taskbar entry, no activation.
    let host = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("LagProbeHiddenHost"),
            w!(""),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            pw,
            ph,
            None,
            None,
            None,
            None,
        )
    }
    .expect("hidden host");
    let surface = NativeVideoWindow::new(host.0 as isize).expect("surface");
    surface.set_bounds(0, 0, pw, ph);
    surface.show(); // child of a hidden parent stays invisible

    let (tx, rx) = mpsc::channel();
    let mut opts = native_mpv_options(surface.wid());
    for (k, v) in opts.iter_mut() {
        if *k == "ao" {
            *v = "null".into(); // silent
        }
    }
    // LAG_MPV_OPTS="video-sync=audio,framedrop=vo": experiment overrides (later entries win in mpv).
    let extra = std::env::var("LAG_MPV_OPTS").unwrap_or_default();
    let extra: Vec<(String, String)> = extra
        .split(',')
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    for (k, v) in &extra {
        println!("override {k}={v}");
        opts.retain(|(ok, _)| ok != k);
    }
    let leaked: Vec<(&'static str, String)> =
        extra.into_iter().map(|(k, v)| (&*Box::leak(k.into_boxed_str()), v)).collect();
    opts.extend(leaked);
    let t0 = Instant::now();
    let player = MpvPlayer::with_options(&opts, move |e| {
        let _ = tx.send(e);
    })
    .expect("mpv native init");
    println!("mpv native init: {} ms", t0.elapsed().as_millis());

    let prop = |n: &str| player.property(n).unwrap_or_else(|_| "-".into());
    for clip in &clips {
        let t = Instant::now();
        player.load(clip).expect("load");
        player.pause(false).expect("play");
        let loaded = wait_for(&rx, Duration::from_secs(10), |e| matches!(e, PlayerEvent::FileLoaded));
        let load_ms = t.elapsed().as_millis();
        pump(Duration::from_secs(secs));
        println!(
            "{}\n  loaded={loaded} in {load_ms} ms | codec={} {}x{} container-fps={} est-vf-fps={} | hwdec-current={} | vo={} \n  time-pos={} drops: frame={} decoder={} vo-delayed={} | display-fps={} mistimed={}",
            std::path::Path::new(clip).file_name().unwrap().to_string_lossy(),
            prop("video-codec"),
            prop("width"),
            prop("height"),
            prop("container-fps"),
            prop("estimated-vf-fps"),
            prop("hwdec-current"),
            prop("current-vo"),
            prop("time-pos"),
            prop("frame-drop-count"),
            prop("decoder-frame-drop-count"),
            prop("vo-delayed-frame-count"),
            prop("display-fps"),
            prop("mistimed-frame-count"),
        );
    }

    // Rapid switching: 12 loads within ~250 ms, the LAST one must be what plays.
    if clips.len() >= 2 {
        let last = clips[clips.len() - 1].clone();
        for i in 0..12 {
            let _ = player.load(&clips[i % clips.len()]);
            pump(Duration::from_millis(20));
        }
        let _ = player.load(&last);
        let _ = player.pause(false);
        let _ = wait_for(&rx, Duration::from_secs(10), |e| matches!(e, PlayerEvent::FileLoaded));
        pump(Duration::from_secs(2));
        let path = prop("path");
        println!(
            "rapid switch: playing={} expected={} -> {} | pause={} time-pos={}",
            std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(path.clone()),
            std::path::Path::new(&last).file_name().unwrap().to_string_lossy(),
            if path == last { "OK" } else { "WRONG CLIP" },
            prop("pause"),
            prop("time-pos"),
        );
    }
    let _ = player.stop();
    drop(player);
    pump(Duration::from_millis(300));
    drop(surface);
    unsafe {
        let _ = DestroyWindow(host);
    }
}
