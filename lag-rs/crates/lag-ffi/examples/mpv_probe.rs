//! Headless libmpv check on a real Lag clip (no window: vo=null, ao=null).
//! Usage: mpv_probe <clip.mp4>
use lag_ffi::mpv::*;
use std::ffi::{CStr, CString};
use std::time::{Duration, Instant};

unsafe fn prop(h: *mut mpv_handle, name: &str) -> String {
    let n = CString::new(name).unwrap();
    let p = mpv_get_property_string(h, n.as_ptr());
    if p.is_null() {
        return "<none>".into();
    }
    let s = CStr::from_ptr(p).to_string_lossy().into_owned();
    mpv_free(p as *mut _);
    s
}

unsafe fn set(h: *mut mpv_handle, k: &str, v: &str) {
    let (k, v) = (CString::new(k).unwrap(), CString::new(v).unwrap());
    let r = mpv_set_option_string(h, k.as_ptr(), v.as_ptr());
    assert!(r >= 0, "option {k:?} failed: {r}");
}

unsafe fn cmd(h: *mut mpv_handle, args: &[&str]) {
    let owned: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();
    let mut ptrs: Vec<*const i8> = owned.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    let r = mpv_command(h, ptrs.as_mut_ptr());
    assert!(r >= 0, "command {args:?} failed: {r}");
}

unsafe fn wait_for(h: *mut mpv_handle, want: mpv_event_id, timeout: Duration) -> Duration {
    let t = Instant::now();
    loop {
        let ev = &*mpv_wait_event(h, 0.05);
        if ev.event_id == want {
            return t.elapsed();
        }
        if ev.event_id == MPV_EVENT_END_FILE || t.elapsed() > timeout {
            panic!("timed out / ended waiting for event {want}");
        }
    }
}

fn main() {
    let path = std::env::args_os().nth(1).expect("clip path");
    let path = path.to_str().expect("utf-8 path").to_owned();
    unsafe {
        let t0 = Instant::now();
        let h = mpv_create();
        set(h, "vo", "null");
        set(h, "ao", "null");
        set(h, "hwdec", &std::env::var("LAG_HWDEC").unwrap_or("d3d11va".into()));
        set(h, "pause", "yes");
        assert_eq!(mpv_initialize(h), 0);
        let init = t0.elapsed();
        cmd(h, &["loadfile", &path]);
        let loaded = wait_for(h, MPV_EVENT_FILE_LOADED, Duration::from_secs(10));
        println!("init {init:?}, file-loaded after {loaded:?}");
        println!(
            "duration {} s, {}x{}, codec {}, fps(container) {}",
            prop(h, "duration"),
            prop(h, "width"),
            prop(h, "height"),
            prop(h, "video-codec"),
            prop(h, "container-fps")
        );
        let n: usize = prop(h, "track-list/count").parse().unwrap_or(0);
        for i in 0..n {
            println!(
                "  track {i}: {} id={} title={} codec={}",
                prop(h, &format!("track-list/{i}/type")),
                prop(h, &format!("track-list/{i}/id")),
                prop(h, &format!("track-list/{i}/title")),
                prop(h, &format!("track-list/{i}/codec"))
            );
        }
        // Play 2 s unpaused to make the decoder start, then report hwdec.
        cmd(h, &["set", "pause", "no"]);
        std::thread::sleep(Duration::from_secs(2));
        println!("hwdec-current {}, frame-drops {}", prop(h, "hwdec-current"), prop(h, "frame-drop-count"));
        // Exact seek latency (editor scrubbing).
        for pos in ["10", "45.5", "3"] {
            let t = Instant::now();
            cmd(h, &["seek", pos, "absolute+exact"]);
            wait_for(h, MPV_EVENT_PLAYBACK_RESTART, Duration::from_secs(5));
            println!("exact seek to {pos}s: {:?} (now at {})", t.elapsed(), prop(h, "time-pos"));
        }
        mpv_terminate_destroy(h);
    }
}
