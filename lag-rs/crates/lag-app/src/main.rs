//! Lag.exe: port of `Program.cs` + `App.axaml.cs` (startup sequence, spec shell.md §4.1).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// The UI comes from .slint files at run time (themes, docs/spec/themes.md): the typed API is the generated twin.
pub use ui_twin::*;
pub use ui_twin::TwinComponent as ComponentHandle;

mod app_ctx;
mod gpucheck;
mod media;
mod services;
mod shell;
mod snapshot;
mod preview;
mod startup;
mod theme_editor;
mod themes;
mod ui_twin;
mod views;

fn main() {
    // Per-monitor DPI awareness before ANY window or monitor query (C# app.manifest PerMonitorV2):
    // without it monitors are reported in logical pixels (3440x1440 @125% -> 2752x1152).
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // The UI is read from ui\*.slint now (themes): compiled before any window exists. Dev modes load it here;
    // the app after the single-instance check (a second launch must not spend ~0.25 s on it).
    let dev_load = || {
        let s = startup::load_settings();
        match themes::activate(s.ui_theme.as_deref().unwrap_or(themes::BUILTIN)) {
            Ok(_) => {
                if let Some(e) = themes::last_error() {
                    eprintln!("theme failed, Lag's own UI is used: {e}");
                }
            }
            Err(e) => eprintln!("UI failed to load: {e}"),
        }
    };
    // Theme preview with live reload: `--theme-preview <theme folder>` (its own UI loading, its own window).
    // Theme editor: `--theme-editor <theme folder>`.
    if let Some(dir) = theme_editor::args() {
        theme_editor::run(dir);
        return;
    }
    if let Some(args) = preview::args() {
        preview::run(args);
        return;
    }
    // Real-window GPU render check (dev tool): `--gpu-check <script> <out-dir>`.
    if let Some((script, dir)) = gpucheck::args() {
        dev_load();
        gpucheck::run(script, dir);
        return;
    }
    // Headless UI snapshot mode (no window, no tray/hotkeys/engine): `--snapshot <out.png> [page] [WxH]`.
    if let Some(snap) = snapshot::install_from_args() {
        dev_load();
        run_snapshot(snap);
        return;
    }
    // 1. Log first, so everything below is captured (FileLog.Initialize).
    let _log = startup::init_logging();
    // 2. 1 ms timer resolution (Program.cs:68).
    startup::raise_timer_resolution();
    // 3. Single instance ("LagAppSingleInstanceMutex"): a second launch exits immediately.
    // after "Restart" (a theme change) the old Lag is still closing: wait for it instead of exiting
    let Some(_instance) = startup::acquire_single_instance_waiting(std::env::args().any(|a| a == "--after-restart")) else {
        log::info!("[Startup] another Lag instance is running \u{2014} exiting.");
        return;
    };
    // 4. Settings + language before any UI string is built.
    let settings = startup::load_settings();
    lag_core::i18n::set_language(settings.language.as_deref().unwrap_or("en"));
    // 4b. The UI from ui\*.slint, and the chosen interface theme over it (docs/spec/themes.md).
    match themes::activate(settings.ui_theme.as_deref().unwrap_or(themes::BUILTIN)) {
        Ok(took) => log::info!(
            "[Startup] UI loaded from {} (theme \"{}\") in {} ms",
            ui_twin::builtin_dir().display(),
            themes::active(),
            took.as_millis()
        ),
        Err(e) => {
            log::error!("[Startup] UI failed to load: {e}");
            startup::fatal_message(&format!("Lag could not load its interface:\n\n{e}"));
            return;
        }
    }

    // The app (main window + context) lives here; built either right away (start minimized: straight to the
    // tray) or behind the startup splash on a manual launch.
    type App = (AppWindow, std::rc::Rc<app_ctx::AppCtx>);
    let app: std::rc::Rc<std::cell::RefCell<Option<App>>> = Default::default();
    let build = |settings: lag_core::settings::SettingsData| -> Option<App> {
        let ui = match AppWindow::new() {
            Ok(ui) => ui,
            Err(e) => {
                log::error!("[Startup] UI init failed: {e}");
                return None;
            }
        };
        ui.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
        ui.global::<MainVM>().set_status_text(lag_core::i18n::tr("Status_Ready").into());
        let ctx = app_ctx::AppCtx::new(settings);
        // the size the user left the window at (settings.json), before it is first shown
        shell::window_geometry::load(&ctx.settings());
        shell::window_geometry::attach(&ctx);
        shell::window_geometry::apply_size(&ui);
        views::init_all(&ui, &ctx);
        shell::init(&ui, &ctx);
        // the real app only: --snapshot / --gpu-check never reach the network or the user's cloud
        ctx.cloud().go_online();
        Some((ui, ctx))
    };

    let mode = settings.theme.clone().unwrap_or_else(|| "dark".into());
    // A newer Lag downloaded earlier (by a start into the tray, or a check the splash gave up on): installed now,
    // before Lag opens; LagUpdate.exe shows it and starts the new Lag.
    if let Some((version, installer)) = shell::updater::cached_update() {
        log::info!("[Startup] installing the downloaded update {}", version.short());
        if shell::updater::hand_off(&installer, &mode) {
            return;
        }
    }
    let splash = (!settings.start_minimized).then(|| startup::show_splash(&settings)).flatten();
    if let Some(splash) = splash {
        // Build inside the event loop so the splash is already animating while the app initialises; keep it
        // up for at least SPLASH_MIN so it reads as an intentional animation, not a flicker.
        const SPLASH_MIN: std::time::Duration = std::time::Duration::from_millis(1400);
        // how long the splash waits for the update check before Lag opens anyway (it goes on in the background)
        const CHECK_WAIT: std::time::Duration = std::time::Duration::from_millis(2500);
        let shown = std::time::Instant::now();
        let splash = std::rc::Rc::new(splash);
        let settings = std::rc::Rc::new(std::cell::RefCell::new(Some(settings)));
        // opens Lag (once): builds it, shows it after SPLASH_MIN, hides the splash
        let open = {
            let (slot, splash, settings) = (app.clone(), splash.clone(), settings.clone());
            std::rc::Rc::new(move || {
                let Some(settings) = settings.borrow_mut().take() else { return };
                splash.set_show_progress(false);
                splash.set_hint("".into());
                splash.set_text(lag_core::i18n::tr("Splash_Starting").into());
                let Some((ui, ctx)) = build(settings) else {
                    let _ = slint::quit_event_loop();
                    return;
                };
                let weak = ui.as_weak();
                *slot.borrow_mut() = Some((ui, ctx));
                let rest = SPLASH_MIN.saturating_sub(shown.elapsed());
                let splash = splash.clone();
                slint::Timer::single_shot(rest, move || {
                    if let Some(ui) = weak.upgrade() {
                        if let Err(e) = ui.show() {
                            log::error!("[Startup] main window show failed: {e}");
                        }
                        shell::window_geometry::apply_state(&ui);
                    }
                    let _ = splash.hide();
                });
            })
        };
        // the update check on the splash (owner: Lag should open already updated)
        let (tx, rx) = std::sync::mpsc::channel();
        let checking = shell::updater::startup_check(move |e| {
            let _ = tx.send(e);
        });
        #[derive(Clone, Copy, PartialEq)]
        enum Phase {
            Checking,
            Updating,
            Opened,
        }
        let phase = std::rc::Rc::new(std::cell::Cell::new(if checking { Phase::Checking } else { Phase::Opened }));
        if !checking {
            let open = open.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(80), move || open());
        }
        {
            // a click on the splash while it downloads: open Lag now, the update installs at the next start
            let (open, phase) = (open.clone(), phase.clone());
            splash.on_skip(move || {
                if phase.get() == Phase::Updating {
                    log::info!("[Startup] update skipped for now: it installs at the next start");
                    phase.set(Phase::Opened);
                    open();
                }
            });
        }
        let version = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let poll = std::rc::Rc::new(slint::Timer::default());
        let (splash2, open2, phase2, me) = (splash.clone(), open.clone(), phase.clone(), std::rc::Rc::downgrade(&poll));
        poll.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(50), move || {
            while let Ok(event) = rx.try_recv() {
                use shell::updater::Startup;
                match (phase2.get(), event) {
                    (Phase::Opened, _) => {}
                    (_, Startup::Nothing) => {
                        phase2.set(Phase::Opened);
                        open2();
                    }
                    (Phase::Checking, Startup::Found(v)) => {
                        log::info!("[Startup] update {} found: downloading it on the splash", v.short());
                        phase2.set(Phase::Updating);
                        *version.borrow_mut() = v.short();
                        splash2.set_show_progress(true);
                        splash2.set_busy(true);
                        splash2.set_text(
                            lag_core::i18n::format("Splash_UpdateDownloading", &[&v.short(), "0"]).unwrap_or_default().into(),
                        );
                        splash2.set_hint(lag_core::i18n::tr("Splash_UpdateSkip").into());
                    }
                    (Phase::Updating, Startup::Progress(p)) => {
                        splash2.set_busy(p.is_none());
                        splash2.set_progress(p.unwrap_or(0.0));
                        let v = version.borrow().clone();
                        let text = match p {
                            Some(p) => lag_core::i18n::format("Splash_UpdateDownloading", &[&v, &((p * 100.0) as i32).to_string()]),
                            None => lag_core::i18n::format("Splash_UpdateDownloadingPlain", &[&v]),
                        };
                        splash2.set_text(text.unwrap_or_default().into());
                    }
                    (Phase::Updating, Startup::Ready(installer)) => {
                        splash2.set_busy(true);
                        splash2.set_hint("".into());
                        splash2.set_text(
                            lag_core::i18n::format("Splash_UpdateInstalling", &[&version.borrow()]).unwrap_or_default().into(),
                        );
                        if shell::updater::hand_off(&installer, &mode) {
                            // LagUpdate.exe takes over the same splash; Lag was never opened, so it just ends
                            let _ = slint::quit_event_loop();
                        } else {
                            phase2.set(Phase::Opened);
                            open2();
                        }
                    }
                    _ => {}
                }
            }
            if phase2.get() == Phase::Checking && shown.elapsed() > CHECK_WAIT {
                log::info!("[Startup] the update check is slow: opening Lag (a newer Lag installs at the next start)");
                phase2.set(Phase::Opened);
                open2();
            }
            // Lag is open: what the check still finds is kept for the next start
            if phase2.get() == Phase::Opened {
                if let Some(t) = me.upgrade() {
                    t.stop();
                }
            }
        });
        // kept until the process ends (it stops itself once Lag is open)
        std::mem::forget(poll);
    } else {
        let Some((ui, ctx)) = build(settings) else { return };
        // NOT ui.run(): that ends the event loop as soon as the last window is hidden, and "start minimized"
        // hides the main window into the tray on the first turn -> the app silently exited at startup. The
        // loop must live until an explicit Exit (tray menu / updater: shell::exit -> quit_event_loop).
        if let Err(e) = ui.show() {
            log::error!("[Startup] main window show failed: {e}");
        }
        shell::window_geometry::apply_state(&ui);
        *app.borrow_mut() = Some((ui, ctx));
    }
    if let Err(e) = slint::run_event_loop_until_quit() {
        log::error!("[Startup] event loop ended with error: {e}");
    }
    let Some((_ui, ctx)) = app.borrow_mut().take() else { return };
    // Orderly exit like C# (Dispose services, then Environment.Exit): recording engine first (bounded wait),
    // then tray/hotkeys/HUD while their threads still exist, settings, log; then end the process without
    // running thread-local destructors, which would otherwise touch threads Windows already tore down.
    views::main_vm::shutdown_recorder();
    shell::window_geometry::save_pending_now();
    ctx.flush_settings(); // C# ShutdownRequested -> FlushSettings()
    shell::shutdown();
    log::info!("[Shutdown] clean exit.");
    drop(_log);
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}

/// `--ui-check` / `--snapshot`: info+ log lines go to stderr (no log file in these modes).
struct StderrLog;
impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("{} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}
static STDERR_LOG: StderrLog = StderrLog;

fn run_snapshot(snap: snapshot::Snapshot) {
    if log::set_logger(&STDERR_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let mut settings = startup::load_settings();
    // A snapshot must never record or pop anything: force the side-effect settings off (in memory only).
    settings.auto_start_recording = false;
    settings.start_minimized = false;
    settings.stats_shown = false;
    lag_core::i18n::set_language(settings.language.as_deref().unwrap_or("en"));
    let Ok(ui) = AppWindow::new() else {
        eprintln!("snapshot: UI init failed");
        return;
    };
    ui.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
    ui.global::<MainVM>().set_status_text(lag_core::i18n::tr("Status_Ready").into());
    let ctx = app_ctx::AppCtx::new(settings);
    // a UI check never writes the user's settings.json (its scripts switch language, theme...)
    ctx.never_save();
    views::init_all(&ui, &ctx);
    let page = match snap.page.as_str() {
        "library" => Page::Library,
        "player" => Page::Player,
        "editor" => Page::Editor,
        _ => Page::Settings,
    };
    ui.global::<MainVM>().set_current_page(page);
    if let Err(e) = ui.show() {
        eprintln!("snapshot: show failed: {e}");
        return;
    }
    if snap.script.is_some() {
        snap.pump(std::time::Duration::from_millis(2500));
        snap.run_script(&ui);
        return;
    }
    match snap.render(std::time::Duration::from_millis(2500)) {
        Ok(()) => println!("snapshot written: {}", snap.out),
        Err(e) => eprintln!("snapshot failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    /// Type-checks the design-system gallery without building it into the exe (and without a window).
    #[test]
    fn gallery_compiles() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/gallery.slint");
        let compiler = slint_interpreter::Compiler::default();
        let result = spin_on(compiler.build_from_path(path));
        let errors: Vec<_> = result
            .diagnostics()
            .filter(|d| d.level() == slint_interpreter::DiagnosticLevel::Error)
            .map(|d| d.to_string())
            .collect();
        assert!(errors.is_empty(), "{errors:#?}");
    }

    fn spin_on<F: std::future::Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(p: *const ()) -> RawWaker {
            RawWaker::new(p, &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut f = std::pin::pin!(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }
}
