//! Headless UI snapshots: `Lag.exe --snapshot <out.png> [settings|library|player|editor] [WxH]`.
//! Scripted UI check: `Lag.exe --ui-check <script.txt> <out-dir>` (see `run_script` for the commands).
//!
//! Renders the real AppWindow with Slint's SOFTWARE renderer into memory and writes a PNG. No native
//! window is created, nothing takes focus: safe to run while the user is in a game. Used to check layout
//! and styling during development. Tray, hotkeys and the recording engine are NOT started in this mode.

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, TargetPixel};
use slint::platform::{Platform, WindowAdapter};
use slint::PhysicalSize;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send>;

struct Queue(Arc<Mutex<Vec<Job>>>);

impl slint::platform::EventLoopProxy for Queue {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        Ok(())
    }
    fn invoke_from_event_loop(&self, event: Job) -> Result<(), slint::EventLoopError> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(event);
        Ok(())
    }
}

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    jobs: Arc<Mutex<Vec<Job>>>,
    /// the first window gets `window`, every later one a window of its own (the theme editor and its hidden
    /// donor AppWindow); off: every window shares `window` (the UI checks)
    separate: bool,
    given: std::cell::Cell<bool>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        if self.separate && self.given.replace(true) {
            let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            w.set_size(PhysicalSize::new(1360, 840));
            return Ok(w);
        }
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
    fn new_event_loop_proxy(&self) -> Option<Box<dyn slint::platform::EventLoopProxy>> {
        Some(Box::new(Queue(self.jobs.clone())))
    }
}

/// 8-bit RGBA target pixel for the software renderer.
#[derive(Clone, Copy, Default)]
struct Rgba([u8; 4]);

impl TargetPixel for Rgba {
    fn blend(&mut self, color: slint::platform::software_renderer::PremultipliedRgbaColor) {
        let a = 255 - color.alpha as u16;
        let mix = |c: u8, s: u8| (s as u16 + (c as u16 * a) / 255) as u8;
        self.0 = [mix(self.0[0], color.red), mix(self.0[1], color.green), mix(self.0[2], color.blue), 255];
    }
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 255])
    }
}

pub struct Snapshot {
    pub out: String,
    pub page: String,
    /// `--ui-check <script> <out-dir>`: run a scripted session instead of one snapshot.
    pub script: Option<(String, String)>,
    cur: std::cell::Cell<(u32, u32)>,
    window: Rc<MinimalSoftwareWindow>,
    jobs: Arc<Mutex<Vec<Job>>>,
}

/// Parses `--snapshot` args and installs the headless platform. Must run before any Slint component exists.
pub fn install_from_args() -> Option<Snapshot> {
    let args: Vec<String> = std::env::args().collect();
    let script = args.iter().position(|a| a == "--ui-check").map(|i| {
        (args.get(i + 1).cloned().unwrap_or_default(), args.get(i + 2).cloned().unwrap_or_else(|| ".".into()))
    });
    let i = match &script {
        Some(_) => args.iter().position(|a| a == "--ui-check")?,
        None => args.iter().position(|a| a == "--snapshot")?,
    };
    let out = if script.is_some() {
        String::new()
    } else {
        args.get(i + 1).cloned().unwrap_or_else(|| "lag-snapshot.png".into())
    };
    let page = args.get(i + 2).cloned().unwrap_or_else(|| "settings".into());
    let size = args
        .get(i + 3)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1360, 840));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(size.0, size.1));
    let jobs = Arc::new(Mutex::new(Vec::new()));
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
        start: Instant::now(),
        jobs: jobs.clone(),
        separate: false,
        given: std::cell::Cell::new(false),
    }))
    .ok()?;
    let size = if script.is_some() { (1360, 840) } else { size };
    let page = if script.is_some() { "settings".into() } else { page };
    window.set_size(PhysicalSize::new(size.0, size.1));
    Some(Snapshot { out, page, script, cur: std::cell::Cell::new(size), window, jobs })
}

/// The theme editor in memory (`--theme-editor <dir> --editor-shots <out-dir>`): its window is drawn by the
/// software renderer, the script's `shot NAME` writes PNGs. No native window: safe while a game runs.
pub fn install_for_editor(w: u32, h: u32) -> Option<Snapshot> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(w, h));
    let jobs = Arc::new(Mutex::new(Vec::new()));
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
        start: Instant::now(),
        jobs: jobs.clone(),
        separate: true,
        given: std::cell::Cell::new(false),
    }))
    .ok()?;
    Some(Snapshot { out: String::new(), page: String::new(), script: None, cur: std::cell::Cell::new((w, h)), window, jobs })
}

impl Snapshot {
    /// Writes what the first window shows now into a PNG.
    pub fn shot(&self, out: &str) -> Result<(), String> {
        self.write_png(out)
    }

    /// `pump` that also draws a frame every 100 ms, as a shown window does: a component made while drawing (the
    /// theme in the editor's ComponentContainer) starts its timers and animations only then.
    pub fn pump_drawing(&self, dur: Duration) {
        let until = Instant::now() + dur;
        let (w, h) = self.cur.get();
        let mut buf = vec![Rgba::default(); (w * h) as usize];
        loop {
            self.window.request_redraw();
            self.window.draw_if_needed(|renderer| {
                renderer.render(&mut buf, w as usize);
            });
            let now = Instant::now();
            if now >= until {
                break;
            }
            self.pump(Duration::from_millis(100).min(until - now));
        }
    }

    /// Runs queued jobs/timers for `settle` (lets background loads land), renders once and writes the PNG.
    pub fn render(&self, settle: Duration) -> Result<(), String> {
        self.pump(settle);
        self.write_png(&self.out)
    }

    /// Runs queued jobs, timers and animations for `dur` (16 ms steps).
    pub fn pump(&self, dur: Duration) {
        let until = Instant::now() + dur;
        loop {
            let jobs: Vec<Job> = std::mem::take(&mut *self.jobs.lock().unwrap_or_else(|e| e.into_inner()));
            for j in jobs {
                j();
            }
            slint::platform::update_timers_and_animations();
            if Instant::now() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn write_png(&self, out: &str) -> Result<(), String> {
        let (w, h) = self.cur.get();
        let mut buf = vec![Rgba::default(); (w * h) as usize];
        self.window.request_redraw();
        self.window.draw_if_needed(|renderer| {
            renderer.render(&mut buf, w as usize);
        });
        let bytes: Vec<u8> = buf.iter().flat_map(|p| p.0).collect();
        image::save_buffer(out, &bytes, w, h, image::ColorType::Rgba8).map_err(|e| e.to_string())
    }

    /// Scripted UI session. All input goes ONLY to this in-memory window (never to the OS).
    /// One command per line, `#` comments:
    ///   scale F (DPI factor, e.g. 1.25) | size WxH | page settings|library|player|editor | tab N (settings tab) | fullscreen 0|1
    ///   lang <code> | choose <setting-id> <index> | recording 0|1 | editor-clip N | move X Y | click X Y | rclick X Y | down X Y | up X Y
    ///   scroll X Y DY | key esc|left|right|space|<text> | wait MS | shot <name>  (-> <out-dir>/<name>.png)
    pub fn run_script(&self, ui: &crate::AppWindow) {
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        use crate::ComponentHandle;
        use slint::LogicalPosition;
        let Some((script, dir)) = &self.script else { return };
        crate::views::player::mark_scripted_input();
        let text = match std::fs::read_to_string(script) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("ui-check: cannot read {script}: {e}");
                return;
            }
        };
        let _ = std::fs::create_dir_all(dir);
        let win = ui.window();
        // `splash`: Lag's startup splash in its update state, drawn over the page (top-left, 300x340)
        let mut splash: Option<crate::SplashWindow> = None;
        let num = |a: &[&str], i: usize| -> f32 { a.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.0) };
        for (ln, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split_whitespace();
            let cmd = it.next().unwrap_or("");
            let a: Vec<&str> = it.collect();
            let at = LogicalPosition::new(num(&a, 0), num(&a, 1));
            match cmd {
                "size" => {
                    let wh = a
                        .first()
                        .and_then(|s| s.split_once('x'))
                        .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
                    if let Some((w, h)) = wh {
                        self.cur.set((w, h));
                        self.window.set_size(PhysicalSize::new(w, h));
                    }
                }
                "page" => {
                    let p = match a.first().copied().unwrap_or("") {
                        "library" => crate::Page::Library,
                        "player" => crate::Page::Player,
                        "editor" => crate::Page::Editor,
                        _ => crate::Page::Settings,
                    };
                    ui.global::<crate::MainVM>().invoke_navigate(p);
                }
                "scale" => win.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: num(&a, 0) }),
                "tab" => ui.global::<crate::SettingsVM>().invoke_select_tab(num(&a, 0) as i32),
                // the Library tab with a connected cloud (in memory only)
                "cloud-preview" => crate::services::cloud::preview_connected(),
                // middle-button autoscroll from a press point (window px): start, pointer, stop
                "autoscroll" => {
                    use crate::shell::autoscroll::{apply, Ui};
                    if a.first().copied() == Some("step") {
                        let g = ui.global::<crate::AutoScroll>();
                        g.set_pending(g.get_pending() + num(&a, 1));
                        continue;
                    }
                    let ui_state = match a.first().copied() {
                        Some("stop") => Ui::Stop,
                        Some("move") => Ui::Move(num(&a, 1), num(&a, 2)),
                        _ => Ui::Start(num(&a, 1), num(&a, 2)),
                    };
                    apply(ui, ui_state);
                }
                "fullscreen" => ui.global::<crate::MainVM>().set_is_fullscreen(a.first() == Some(&"1")),
                "recording" => ui.global::<crate::MainVM>().set_is_recording(a.first() == Some(&"1")),
                // the record / save buttons while their action is under way
                "starting" => ui.global::<crate::MainVM>().set_is_starting(a.first() == Some(&"1")),
                "saving" => ui.global::<crate::MainVM>().set_is_saving(a.first() == Some(&"1")),
                // open the N-th clip of the editor's clip list (probe, filmstrip, audio lanes; no video headless)
                "editor-clip" => {
                    use slint::Model;
                    let vm = ui.global::<crate::EditorVM>();
                    if let Some(c) = vm.get_clips().row_data(num(&a, 0) as usize) {
                        vm.invoke_open_clip(c.path);
                    }
                }
                // clip merging: `editor-merge` toggles picking mode, `editor-merge-pick N` picks/unpicks clip N
                "editor-merge" => ui.global::<crate::EditorVM>().invoke_toggle_merge_mode(),
                "editor-merge-pick" => {
                    use slint::Model;
                    let vm = ui.global::<crate::EditorVM>();
                    if let Some(c) = vm.get_clips().row_data(num(&a, 0) as usize) {
                        vm.invoke_merge_pick(c.path);
                    }
                }
                // montage: `editor-montage-run` = "Montage (N)", `editor-segment N` selects, `editor-scrub F` puts
                // the playhead at fraction F of the clip, `editor-split` = razor, `editor-add N` = sidebar "+"
                "editor-montage-run" => ui.global::<crate::EditorVM>().invoke_merge_clips(),
                "editor-segment" => ui.global::<crate::EditorVM>().invoke_select_segment(num(&a, 0) as i32),
                "editor-scrub" => {
                    let vm = ui.global::<crate::EditorVM>();
                    vm.invoke_timeline_drag(3, num(&a, 0), 800.0);
                    vm.invoke_timeline_drag_end();
                }
                "editor-split" => ui.global::<crate::EditorVM>().invoke_split_segment(),
                "editor-add" => {
                    use slint::Model;
                    let vm = ui.global::<crate::EditorVM>();
                    if let Some(c) = vm.get_clips().row_data(num(&a, 0) as usize) {
                        vm.invoke_add_to_montage(c.path);
                    }
                }
                // Same path as the language picker in Settings (persists to the scratch settings only).
                // splash MODE STATE: STATE = download (42 %), unknown (no size), install
                "splash" => {
                    if let Ok(sp) = crate::SplashWindow::new() {
                        let tr = lag_core::i18n::format;
                        sp.set_theme_mode(a.first().copied().unwrap_or("dark").into());
                        sp.set_show_progress(true);
                        match a.get(1).copied().unwrap_or("download") {
                            "install" => {
                                sp.set_busy(true);
                                sp.set_text(tr("Splash_UpdateInstalling", &["2.2.1"]).unwrap_or_default().into());
                            }
                            "unknown" => {
                                sp.set_busy(true);
                                sp.set_text(tr("Splash_UpdateDownloadingPlain", &["2.2.1"]).unwrap_or_default().into());
                                sp.set_hint(lag_core::i18n::tr("Splash_UpdateSkip").into());
                            }
                            _ => {
                                sp.set_progress(0.42);
                                sp.set_text(tr("Splash_UpdateDownloading", &["2.2.1", "42"]).unwrap_or_default().into());
                                sp.set_hint(lag_core::i18n::tr("Splash_UpdateSkip").into());
                            }
                        }
                        let _ = sp.show();
                        splash = Some(sp);
                    }
                }
                "splash-off" => {
                    if let Some(sp) = splash.take() {
                        let _ = sp.hide();
                    }
                }
                "lang" => {
                    let code = a.first().copied().unwrap_or("en");
                    let idx = lag_core::i18n::LANGUAGES.iter().position(|l| l.code == code).unwrap_or(0);
                    ui.global::<crate::SettingsVM>().invoke_choose("language".into(), idx as i32);
                }
                "choose" => ui
                    .global::<crate::SettingsVM>()
                    .invoke_choose(a.first().copied().unwrap_or("").into(), num(&a, 1) as i32),
                "move" => win.dispatch_event(WindowEvent::PointerMoved { position: at }),
                "down" => {
                    win.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left })
                }
                "up" => {
                    win.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left })
                }
                "click" | "rclick" => {
                    let b = if cmd == "click" { PointerEventButton::Left } else { PointerEventButton::Right };
                    win.dispatch_event(WindowEvent::PointerMoved { position: at });
                    win.dispatch_event(WindowEvent::PointerPressed { position: at, button: b });
                    win.dispatch_event(WindowEvent::PointerReleased { position: at, button: b });
                }
                "scroll" => {
                    win.dispatch_event(WindowEvent::PointerScrolled { position: at, delta_x: 0.0, delta_y: num(&a, 2) })
                }
                "key" => {
                    let t: slint::SharedString = match a.first().copied().unwrap_or("") {
                        "esc" => Key::Escape.into(),
                        "left" => Key::LeftArrow.into(),
                        "right" => Key::RightArrow.into(),
                        "space" => " ".into(),
                        other => other.into(),
                    };
                    win.dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
                    win.dispatch_event(WindowEvent::KeyReleased { text: t });
                }
                "wait" => self.pump(Duration::from_millis(num(&a, 0) as u64)),
                // `bench-scroll X Y STEPS DY`: STEPS wheel steps at X,Y; after each, frames are drawn for 50 ms and
                // timed (the UI's work + drawing). Prints the frame times (a smoothness check without a window).
                "bench-scroll" => {
                    let (steps, dy) = (num(&a, 2).max(1.0) as usize, num(&a, 3));
                    let (w, h) = self.cur.get();
                    let mut buf = vec![Rgba::default(); (w * h) as usize];
                    let mut times: Vec<f64> = Vec::new();
                    let started = Instant::now();
                    for _ in 0..steps {
                        win.dispatch_event(WindowEvent::PointerScrolled { position: at, delta_x: 0.0, delta_y: dy });
                        let until = Instant::now() + Duration::from_millis(50);
                        while Instant::now() < until {
                            let t = Instant::now();
                            slint::platform::update_timers_and_animations();
                            self.window.request_redraw();
                            self.window.draw_if_needed(|r| {
                                r.render(&mut buf, w as usize);
                            });
                            times.push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    times.sort_by(|x, y| x.total_cmp(y));
                    let pick = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
                    println!(
                        "bench-scroll: {} frames in {:.0} ms | median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
                        times.len(),
                        started.elapsed().as_secs_f64() * 1000.0,
                        pick(0.5),
                        pick(0.95),
                        pick(1.0)
                    );
                }
                "shot" => {
                    self.pump(Duration::from_millis(50));
                    let out = format!("{dir}/{}.png", a.first().copied().unwrap_or("shot"));
                    match self.write_png(&out) {
                        Ok(()) => println!("shot {out}"),
                        Err(e) => eprintln!("ui-check: shot failed: {e}"),
                    }
                }
                _ => eprintln!("ui-check: line {}: unknown command `{cmd}`", ln + 1),
            }
            self.pump(Duration::from_millis(16));
        }
    }
}
