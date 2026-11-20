//! Mirrors ViewModels/MainViewModel.cs and Views/MainWindow.axaml.cs: shell and recording commands.
use crate::app_ctx::AppCtx;
use crate::{
    services::{
        nofocus,
        recorder::{self, Event, RecorderService},
        screenshot, sound, toast, AppCtxPlaybackExt,
    },
    AppWindow, MainVM, Page,
};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::{Input::KeyboardAndMouse::ReleaseCapture, WindowsAndMessaging::*},
};

const TOGGLE_DEBOUNCE: Duration = Duration::from_millis(1200);
#[derive(Default)]
struct Debounce {
    last: Option<Instant>,
}
impl Debounce {
    fn accept(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|last| now.saturating_duration_since(last) < TOGGLE_DEBOUNCE) {
            return false;
        }
        self.last = Some(now);
        true
    }
}
#[derive(Default)]
struct Buffer {
    elapsed: i32,
    total: i32,
}
impl Buffer {
    fn start(&mut self, total: i32) {
        self.elapsed = 0;
        self.total = total.max(1);
    }
    fn tick(&mut self) {
        self.elapsed = (self.elapsed + 1).min(self.total);
    }
    fn text(&self) -> String {
        if self.total == 0 {
            "0:00".into()
        } else {
            format!("{} / {}", fmt_seconds(self.elapsed), fmt_seconds(self.total))
        }
    }
    fn percent(&self) -> f32 {
        if self.total == 0 {
            0.
        } else {
            100. * self.elapsed as f32 / self.total as f32
        }
    }
}
fn fmt_seconds(seconds: i32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn tr(key: &str) -> String {
    lag_core::i18n::tr(key).into()
}
fn formatted(key: &str, value: &str) -> String {
    lag_core::i18n::format(key, &[value]).unwrap_or_else(|_| tr(key))
}
fn filename(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
fn notify(key: &str, subtitle: String, icon: &str, accent: &str) {
    toast::show(tr(key), subtitle, icon, accent);
}
/// Toast only for real target changes: entering a game, switching to another game, or the game closing.
/// Alt+Tab (game -> desktop while the game still runs, and back) is not news (owner, 2026-11-16).
fn target_toast(previous: &mut Option<(bool, String)>, target: (bool, String), previous_game_running: bool) -> bool {
    let Some(prev) = previous.as_ref() else {
        *previous = Some(target);
        return false;
    };
    if *prev == target {
        return false;
    }
    if !target.0 && prev.0 && previous_game_running {
        return false;
    } // alt-tab out: remember the game
    *previous = Some(target);
    true
}

struct Runtime {
    ui: slint::Weak<AppWindow>,
    ctx: Weak<AppCtx>,
    recorder: RefCell<Option<RecorderService>>,
    starting: Cell<bool>,
    debounce: RefCell<Debounce>,
    buffer: RefCell<Buffer>,
    last_target: RefCell<Option<(bool, String)>>,
    last_game_exe: RefCell<Option<String>>,
    ticker: Timer,
    watchdog: Timer,
    /// Current status as (resource key, optional argument) so a language change can re-translate it.
    status_src: RefCell<(String, Option<String>)>,
}
impl Runtime {
    fn with_recorder(&self, f: impl FnOnce(&RecorderService)) {
        if let Some(r) = &*self.recorder.borrow() {
            f(r);
        }
    }
    fn status(&self, key: &str) {
        self.status_with(key, None);
    }
    fn status_with(&self, key: &str, arg: Option<String>) {
        let text = match &arg {
            Some(a) => formatted(key, a),
            None => tr(key),
        };
        *self.status_src.borrow_mut() = (key.into(), arg);
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<MainVM>().set_status_text(text.into());
        }
    }
    fn retranslate_status(&self) {
        let (key, arg) = self.status_src.borrow().clone();
        self.status_with(&key, arg);
    }
    fn update_buffer(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let vm = ui.global::<MainVM>();
            let buffer = self.buffer.borrow();
            vm.set_buffer_status(buffer.text().into());
            vm.set_buffer_fill_percent(buffer.percent());
        }
    }
    fn toggle(&self) {
        if self.starting.get() || !self.debounce.borrow_mut().accept(Instant::now()) {
            return;
        }
        let (Some(ui), Some(ctx)) = (self.ui.upgrade(), self.ctx.upgrade()) else {
            return;
        };
        let vm = ui.global::<MainVM>();
        self.last_target.borrow_mut().take();
        if vm.get_is_recording() {
            vm.set_is_recording(false);
            vm.set_is_paused(false);
            vm.set_has_capture_target(false);
            self.status("Status_Stopping");
            *self.buffer.borrow_mut() = Buffer::default();
            self.update_buffer();
            notify("Toast_RecordingStopped", String::new(), "stop-circle", "Fg2Brush");
            self.with_recorder(RecorderService::stop);
        } else {
            self.starting.set(true);
            vm.set_is_starting(true);
            self.status("Status_Starting");
            // Commands are serialized: an in-flight Stop always finishes before the next Start.
            self.with_recorder(|r| r.start(ctx.settings()));
        }
    }
    fn event(&self, event: Event) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let vm = ui.global::<MainVM>();
        match event {
            Event::Started(result) => {
                self.starting.set(false);
                vm.set_is_starting(false);
                match result {
                    Ok(total) => {
                        vm.set_is_recording(true);
                        vm.set_is_paused(false);
                        self.status("Status_Recording");
                        self.buffer.borrow_mut().start(total);
                        self.update_buffer();
                        self.last_target.borrow_mut().take();
                        self.with_recorder(RecorderService::poll);
                        log::info!("[DEBUG] Recording started! IsRecording=True");
                        notify("Toast_RecordingStarted", String::new(), "record-circle", "RecDot");
                    }
                    Err(error) => {
                        log::info!("[DEBUG] StartRecording FAILED: {error}");
                        self.status_with("Status_RecordError", Some(error.to_string()));
                    }
                }
            }
            Event::Stopped => {
                if !self.starting.get() && !vm.get_is_recording() {
                    self.status("Status_Stopped");
                }
            }
            Event::Status(status) => {
                crate::shell::hud::update_frames(status.frames_captured);
                self.target(status, false);
            }
            Event::Target(status) => {
                crate::shell::hud::update_frames(status.frames_captured);
                self.target(status, true);
            }
            Event::ReplaySaved(path) => {
                self.watchdog.stop();
                vm.set_is_saving(false);
                self.status_with("Status_ReplaySaved", Some(filename(&path)));
                // Recorder wrote the metadata sidecar off-thread BEFORE this refresh.
                if let Some(ctx) = self.ctx.upgrade() {
                    ctx.request_library_refresh();
                    // the cloud's upload rule decides whether it goes up (docs/spec/cloud.md §7)
                    ctx.cloud().clip_saved(&path);
                }
                notify("Toast_ReplaySaved", filename(&path), "check-circle", "Accent");
            }
        }
    }
    fn target(&self, status: lag_capture::engine::EngineStatus, reconcile_pause: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let vm = ui.global::<MainVM>();
        if !vm.get_is_recording() && !self.starting.get() {
            vm.set_has_capture_target(false);
            return;
        }
        // Reconcile engine auto-switch resets. Only change pause UI when it differs.
        if reconcile_pause && vm.get_is_paused() != status.paused {
            vm.set_is_paused(status.paused);
            self.status(if status.paused { "Status_Paused" } else { "Status_Recording" });
        }
        vm.set_has_capture_target(status.target_active);
        if !status.target_active {
            return;
        }
        let name = if status.target_is_game && !status.target_name.trim().is_empty() {
            lag_core::metadata::normalize_game_name(&status.target_name)
        } else {
            tr("Library_Desktop")
        };
        vm.set_capture_target_is_game(status.target_is_game);
        vm.set_capture_target_name(name.clone().into());
        if status.target_is_game {
            *self.last_game_exe.borrow_mut() = status.target_exe.clone();
        }
        let prev_is_game = self.last_target.borrow().as_ref().is_some_and(|t| t.0);
        let game_running = !status.target_is_game
            && prev_is_game
            && self.last_game_exe.borrow().as_deref().is_some_and(|e| lag_audio::sessions::find_process(e).is_some());
        let changed =
            target_toast(&mut self.last_target.borrow_mut(), (status.target_is_game, name.clone()), game_running);
        if vm.get_is_recording() && changed {
            notify("Toast_NowRecording", name, if status.target_is_game { "controller" } else { "monitor" }, "Accent");
        }
    }
    fn pause(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let vm = ui.global::<MainVM>();
        if !vm.get_is_recording() {
            return;
        }
        let paused = !vm.get_is_paused();
        vm.set_is_paused(paused);
        self.status(if paused { "Status_Paused" } else { "Status_Recording" });
        self.with_recorder(|r| r.pause(paused));
    }
    fn save(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let vm = ui.global::<MainVM>();
        log::info!(
            "[DEBUG] SaveReplay() EXECUTING! IsRecording={}, IsSaving={}",
            vm.get_is_recording(),
            vm.get_is_saving()
        );
        if !vm.get_is_recording() {
            self.status("Status_NotRecording");
            log::info!("[DEBUG] SaveReplay() BLOCKED: Not recording.");
            return;
        }
        if vm.get_is_saving() {
            log::info!("[DEBUG] SaveReplay() BLOCKED: Already saving.");
            return;
        }
        vm.set_is_saving(true);
        self.status("Status_Saving");
        notify("Status_Saving", String::new(), "content-save", "Accent");
        sound::play("save.wav");
        let me = Rc::downgrade(self);
        // Restartable timer: a completed save's watchdog cannot reset a newer in-flight save.
        self.watchdog.start(TimerMode::SingleShot, Duration::from_secs(120), move || {
            if let Some(me) = me.upgrade() {
                if let Some(ui) = me.ui.upgrade() {
                    let vm = ui.global::<MainVM>();
                    if vm.get_is_saving() {
                        vm.set_is_saving(false);
                        me.status_with("Status_SaveError", Some("timeout".into()));
                    }
                }
            }
        });
        self.with_recorder(RecorderService::save);
    }
    fn screenshot(&self) {
        let Some(ctx) = self.ctx.upgrade() else {
            return;
        };
        let settings = ctx.settings();
        std::thread::spawn(move || {
            let devices =
                recorder::Devices { monitors: lag_core::hardware::get_available_monitors(), ..Default::default() };
            let device = recorder::selected_monitor(&settings, &devices).map(|m| m.device_name.clone());
            if let Some(path) = screenshot::capture(device, recorder::library_path(&settings)) {
                sound::play("screenshot.wav");
                let _ = slint::invoke_from_event_loop(move || {
                    crate::services::cloud::clip_saved(&path.to_string_lossy());
                    notify("Toast_ScreenshotSaved", filename(&path.to_string_lossy()), "check-circle", "Accent")
                });
            }
        });
    }
    fn navigate(&self, page: Page) {
        let (Some(ui), Some(ctx)) = (self.ui.upgrade(), self.ctx.upgrade()) else {
            return;
        };
        let vm = ui.global::<MainVM>();
        if vm.get_current_page() == Page::Player && page != Page::Player {
            ctx.stop_playback(false);
        }
        vm.set_current_page(page);
        if page != Page::Settings {
            ctx.request_library_refresh();
        }
    }
    fn close(&self) {
        if let Some(ctx) = self.ctx.upgrade() {
            ctx.stop_playback(true);
        }
        if let Some(ui) = self.ui.upgrade() {
            let _ = ui.hide();
        }
    }
    fn fullscreen(&self) {
        let weak = self.ui.clone();
        // Video fades to black first, then the window switches; it fades back in once resized (player.rs).
        crate::views::player::toggle_fullscreen_with_fade(Box::new(move || {
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<MainVM>();
                let full = !vm.get_is_fullscreen();
                vm.set_is_fullscreen(full);
                set_window_chrome(&ui, full);
                ui.window().set_fullscreen(full);
                if !full {
                    ui.window().set_maximized(false);
                } // C# restores Normal, including from maximized.
            }
        }));
    }
}

/// DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND on the main window. False while it has no HWND yet.
fn apply_round_corners(ui: &AppWindow) -> bool {
    set_window_chrome(ui, ui.global::<MainVM>().get_is_fullscreen())
}

/// Windowed: rounded corners + the system 1 px border. Fullscreen: square corners and no border (owner,
/// 2026-11-17: "fullscreen is not quite full, a pixel or two strip around the whole window" - Win11 keeps
/// rounding and outlining a borderless-fullscreen window that is not maximized).
fn set_window_chrome(ui: &AppWindow, fullscreen: bool) -> bool {
    let Some(hwnd) = nofocus::hwnd(ui.window()) else { return false };
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    };
    let pref = if fullscreen { DWMWCP_DONOTROUND } else { DWMWCP_ROUND };
    // DWMWA_COLOR_NONE = 0xFFFFFFFE, DWMWA_COLOR_DEFAULT = 0xFFFFFFFF
    let border: u32 = if fullscreen { 0xFFFF_FFFE } else { 0xFFFF_FFFF };
    let ok = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        )
    }
    .is_ok();
    let _ = unsafe { DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4) };
    if !ok {
        log::warn!("[Shell] window corners: DwmSetWindowAttribute failed");
    }
    ok
}

/// Frameless window resize zone (owner, 2026-11-17: "you have to catch one stupid pixel to resize"). The
/// system frame used to give an ~8 px grab border; without it only the 1 px edge resized. Hit-testing the
/// outer 8 px (DPI-scaled) of the client area as edges/corners restores it. Not when maximized/fullscreen.
unsafe extern "system" fn resize_border_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;
    let r = unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
    if msg != WM_NCHITTEST || r.0 != HTCLIENT as isize {
        return r;
    }
    unsafe {
        if IsZoomed(hwnd).as_bool() {
            return r;
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if style & WS_SIZEBOX.0 == 0 {
            return r;
        } // fullscreen / not resizable
        let mut rc = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(hwnd, &mut rc).is_err() {
            return r;
        }
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as i32;
        let b = 8 * dpi / 96;
        let x = (lp.0 & 0xFFFF) as i16 as i32;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
        let (l, t, rr, bb) = (x < rc.left + b, y < rc.top + b, x >= rc.right - b, y >= rc.bottom - b);
        let code = match (l, t, rr, bb) {
            (true, true, _, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, _, true, true) => HTBOTTOMRIGHT,
            (true, _, _, _) => HTLEFT,
            (_, _, true, _) => HTRIGHT,
            (_, true, _, _) => HTTOP,
            (_, _, _, true) => HTBOTTOM,
            _ => return r,
        };
        windows::Win32::Foundation::LRESULT(code as isize)
    }
}
fn install_resize_border(ui: &AppWindow) -> bool {
    let Some(hwnd) = nofocus::hwnd(ui.window()) else { return false };
    unsafe { windows::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(resize_border_proc), 0x4c4152, 0).as_bool() }
}

thread_local! { static RUNTIME: RefCell<std::rc::Weak<Runtime>> = const { RefCell::new(std::rc::Weak::new()) }; }
/// Called by main after the event loop ends: stop recording before the process goes away.
pub fn shutdown_recorder() {
    let runtime = RUNTIME.with(|r| r.borrow().upgrade());
    if let Some(rt) = runtime {
        if let Some(r) = rt.recorder.borrow_mut().take() {
            let t = Instant::now();
            let ok = r.shutdown_blocking(Duration::from_secs(3));
            log::info!("[Shutdown] recorder stopped={ok} in {} ms", t.elapsed().as_millis());
        }
    }
}

pub fn init(ui: &AppWindow, ctx: &Rc<AppCtx>) {
    let runtime = Rc::new(Runtime {
        ui: ui.as_weak(),
        ctx: Rc::downgrade(ctx),
        recorder: RefCell::default(),
        starting: Cell::new(false),
        debounce: RefCell::default(),
        buffer: RefCell::default(),
        last_target: RefCell::default(),
        last_game_exe: RefCell::default(),
        ticker: Timer::default(),
        watchdog: Timer::default(),
        status_src: RefCell::new(("Status_Ready".into(), None)),
    });
    RUNTIME.with(|r| *r.borrow_mut() = Rc::downgrade(&runtime));
    let weak = Rc::downgrade(&runtime);
    *runtime.recorder.borrow_mut() = Some(RecorderService::new(move |event| {
        if let Some(me) = weak.upgrade() {
            me.event(event);
        }
    }));
    // Stats HUD FPS reads the engine's counter directly, like C# (owner, 2026-11-17: the HUD showed CPU/GPU/RAM
    // but always "FPS --"; the counter only moved with queued status events that a busy engine state lock held up)
    runtime.with_recorder(|r| crate::shell::hud::set_frame_counter(r.frame_counter()));
    let vm = ui.global::<MainVM>();
    vm.set_current_page(Page::Settings);
    vm.set_status_text(tr("Status_Ready").into());
    // Status pill follows a language change like every other string (it is set from Rust, not bound to Tr).
    let lang = RefCell::new(ctx.settings().language);
    let me = Rc::downgrade(&runtime);
    ctx.on_settings_changed(move |s| {
        if *lang.borrow() != s.language {
            *lang.borrow_mut() = s.language.clone();
            // Deferred: the settings view's listener (registered later) is the one that switches i18n.
            let me = me.clone();
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(me) = me.upgrade() {
                    me.retranslate_status();
                }
            });
        }
    });
    let me = runtime.clone();
    vm.on_toggle_recording(move || me.toggle());
    let me = runtime.clone();
    vm.on_toggle_pause_recording(move || me.pause());
    let me = runtime.clone();
    vm.on_save_replay(move || me.save());
    let me = runtime.clone();
    vm.on_take_screenshot(move || me.screenshot());
    let me = runtime.clone();
    vm.on_navigate(move |page| me.navigate(page));
    let me = runtime.clone();
    vm.on_close_window(move || me.close());
    let me = runtime.clone();
    vm.on_toggle_fullscreen(move || me.fullscreen());
    let me = runtime.clone();
    vm.on_push_to_talk(move |pressed| {
        if let (Some(ui), Some(ctx)) = (me.ui.upgrade(), me.ctx.upgrade()) {
            if ui.global::<MainVM>().get_is_recording() && ctx.settings().ptt_enabled {
                me.with_recorder(|r| r.mute_mic(!pressed));
            }
        }
    });
    // Push-to-talk switched DURING a recording (settings audit 2026-11-17): the session started with the mic
    // muted (mic_start_muted = ptt_enabled), so turning PTT off left the mic dead until the next restart (C# too;
    // the old "release" call here was skipped because the settings already said PTT off). Off -> mic live now,
    // on -> muted until the key is held.
    {
        let me = Rc::downgrade(&runtime);
        let last = Cell::new(ctx.settings().ptt_enabled);
        ctx.on_settings_changed(move |s| {
            if last.replace(s.ptt_enabled) == s.ptt_enabled {
                return;
            }
            let Some(me) = me.upgrade() else { return };
            if me.ui.upgrade().is_some_and(|ui| ui.global::<MainVM>().get_is_recording()) {
                me.with_recorder(|r| r.mute_mic(s.ptt_enabled));
                log::info!(
                    "[PTT] switched {} during recording -> mic {}",
                    if s.ptt_enabled { "on" } else { "off" },
                    if s.ptt_enabled { "muted" } else { "live" }
                );
            }
        });
    }
    let weak = ui.as_weak();
    vm.on_minimize_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let weak = ui.as_weak();
    vm.on_maximize_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_maximized(!ui.window().is_maximized());
        }
    });
    // Window drag from the custom title bar. Windows' modal move loop (WM_NCLBUTTONDOWN/HTCAPTION) swallows
    // the mouse-up, so Slint would keep the pointer "grabbed" by the drag TouchArea and route EVERY later click
    // to it (whole window draggable, nothing clickable). So: start the move after Slint finished handling the
    // press, then hand Slint the missing release.
    let weak = ui.as_weak();
    vm.on_begin_move_drag(move || {
        let weak = weak.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(hwnd) = nofocus::hwnd(ui.window()) {
                unsafe {
                    let _ = ReleaseCapture();
                    SendMessageW(hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(LPARAM(0)));
                }
            }
            let pos = slint::LogicalPosition::new(-1.0, -1.0);
            ui.window().dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: pos,
                button: slint::platform::PointerEventButton::Left,
            });
            ui.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
        });
    });
    // Windows 11 rounded corners for the frameless window (Avalonia's ExtendClientArea got them from the system
    // frame). The HWND only exists once the window is first shown: with "start minimized" the app starts in the
    // tray, so a one-shot at startup found no window and the corners stayed square. Re-apply until it sticks
    // (cheap check, stops itself), and on every show from the tray.
    {
        let weak = ui.as_weak();
        let corners = Rc::new(Timer::default());
        let keep = corners.clone();
        corners.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            let _ = &keep;
            if let Some(ui) = weak.upgrade() {
                if ui.window().is_visible() && apply_round_corners(&ui) {
                    install_resize_border(&ui);
                    keep.stop();
                }
            }
        });
        std::mem::forget(corners); // lives for the app; stopped once applied
    }
    let weak = ui.as_weak();
    vm.on_show_main_window(move || {
        if let Some(ui) = weak.upgrade() {
            // the window may come back as a fresh native window: give it the remembered size first
            crate::shell::window_geometry::apply_size(&ui);
            let _ = ui.show();
            ui.window().set_minimized(false);
            crate::shell::window_geometry::apply_state(&ui);
            apply_round_corners(&ui);
            if let Some(hwnd) = nofocus::hwnd(ui.window()) {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
        }
    });
    let me = Rc::downgrade(&runtime);
    ui.window().on_close_requested(move || {
        if let Some(me) = me.upgrade() {
            me.close();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let me = Rc::downgrade(&runtime);
    // Player keys at window level (owner, 2026-11-17: none worked): Slint's inner focus on the player page could
    // be lost, so Space / M / arrows / Esc are handled here whenever the player page is up, then swallowed.
    ui.window().on_winit_window_event(move |_, event| {
        use winit::keyboard::{Key, NamedKey};
        if let winit::event::WindowEvent::Resized(size) = event {
            if let Some(ui) = me.upgrade().and_then(|me| me.ui.upgrade()) {
                let full = ui.global::<MainVM>().get_is_fullscreen();
                crate::shell::window_geometry::on_resized(&ui, full, (size.width, size.height));
            }
            return EventResult::Propagate;
        }
        if let winit::event::WindowEvent::KeyboardInput { event, .. } = event {
            if event.state != winit::event::ElementState::Pressed {
                return EventResult::Propagate;
            }
            let Some(me) = me.upgrade() else {
                return EventResult::Propagate;
            };
            let Some(ui) = me.ui.upgrade() else {
                return EventResult::Propagate;
            };
            let vm = ui.global::<MainVM>();
            if event.logical_key == Key::Named(NamedKey::Escape) && vm.get_is_fullscreen() {
                me.fullscreen();
                return EventResult::PreventDefault;
            }
            if vm.get_current_page() == Page::Player {
                let p = ui.global::<crate::PlayerVM>();
                match &event.logical_key {
                    Key::Named(NamedKey::Space) => {
                        if !event.repeat {
                            p.invoke_toggle();
                        }
                        return EventResult::PreventDefault;
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        p.invoke_skip(-10.0);
                        return EventResult::PreventDefault;
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        p.invoke_skip(10.0);
                        return EventResult::PreventDefault;
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("m") || c.as_str() == "ь" || c.as_str() == "Ь" => {
                        if !event.repeat {
                            p.invoke_mute();
                        }
                        return EventResult::PreventDefault;
                    }
                    // F toggles fullscreen (owner, 2026-11-17); "а" is the same key on the Ukrainian layout
                    Key::Character(c) if c.eq_ignore_ascii_case("f") || c.as_str() == "а" || c.as_str() == "А" => {
                        if !event.repeat && p.get_has_clip() {
                            me.fullscreen();
                        }
                        return EventResult::PreventDefault;
                    }
                    _ => {}
                }
            }
        }
        EventResult::Propagate
    });
    let weak = ui.as_weak();
    ctx.on_play_clip(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<MainVM>().set_current_page(Page::Player);
        }
    });
    let me = Rc::downgrade(&runtime);
    ctx.on_edit_clip(move |_| {
        if let Some(me) = me.upgrade() {
            me.navigate(Page::Editor);
        }
    });
    let weak = ui.as_weak();
    ctx.on_settings_changed(move |s| {
        let mode = s.theme.as_deref().unwrap_or("dark");
        toast::set_theme(mode);
        if let Some(ui) = weak.upgrade() {
            if ui.get_theme_mode().as_str() != mode {
                theme_crossfade(&ui);
            }
            ui.set_theme_mode(mode.into());
        }
    });
    let settings = ctx.settings();
    ui.set_theme_mode(settings.theme.as_deref().unwrap_or("dark").into());
    toast::set_theme(settings.theme.as_deref().unwrap_or("dark"));
    let me = Rc::downgrade(&runtime);
    runtime.ticker.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(me) = me.upgrade() {
            if let Some(ui) = me.ui.upgrade() {
                let vm = ui.global::<MainVM>();
                if vm.get_is_recording() && !vm.get_is_paused() {
                    me.buffer.borrow_mut().tick();
                    me.update_buffer();
                }
                me.with_recorder(RecorderService::poll);
                if ui.window().is_fullscreen() != vm.get_is_fullscreen() {
                    set_window_chrome(&ui, vm.get_is_fullscreen());
                    ui.window().set_fullscreen(vm.get_is_fullscreen());
                }
            }
        }
    });
    // init_all constructs MainVM before the page VMs register their listeners.
    let weak = Rc::downgrade(ctx);
    Timer::single_shot(Duration::ZERO, move || {
        if let Some(ctx) = weak.upgrade() {
            ctx.request_library_refresh();
        }
    });
    if settings.auto_start_recording {
        let me = Rc::downgrade(&runtime);
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(me) = me.upgrade() {
                me.toggle();
            }
        });
    }
}

/// Owner, 2026-11-17: "a smooth transition between themes, not a flash over half the screen". The current frame
/// is captured, laid over the window, the theme switches underneath, and the snapshot fades out (380 ms).
/// Headless/minimized/no snapshot -> the theme just switches.
fn theme_crossfade(ui: &AppWindow) {
    if !ui.window().is_visible() || ui.window().is_minimized() {
        return;
    }
    let Ok(shot) = ui.window().take_snapshot() else { return };
    let vm = ui.global::<MainVM>();
    vm.set_theme_fade_image(slint::Image::from_rgba8(shot));
    vm.set_theme_fade_shown(true);
    vm.set_theme_fade_active(true);
    let weak = ui.as_weak();
    // next frame: start fading (the overlay must be drawn once at full opacity over the new theme first)
    Timer::single_shot(Duration::from_millis(34), move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<MainVM>().set_theme_fade_shown(false);
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_millis(420), move || {
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<MainVM>();
                if !vm.get_theme_fade_shown() {
                    vm.set_theme_fade_active(false);
                    vm.set_theme_fade_image(slint::Image::default());
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buffer_formats_minutes_and_holds_at_full() {
        let mut b = Buffer::default();
        assert_eq!(b.text(), "0:00");
        assert_eq!(b.percent(), 0.);
        b.start(300);
        assert_eq!(b.text(), "0:00 / 5:00");
        for _ in 0..155 {
            b.tick();
        }
        assert_eq!(b.text(), "2:35 / 5:00");
        assert!((b.percent() - 51.666668).abs() < 0.001);
        for _ in 0..500 {
            b.tick();
        }
        assert_eq!(b.text(), "5:00 / 5:00");
        assert_eq!(b.percent(), 100.);
        b.start(0);
        b.tick();
        assert_eq!(b.text(), "0:01 / 0:01");
        assert_eq!(fmt_seconds(3600), "60:00");
    }
    #[test]
    fn debounce_accepts_first_and_exact_boundary_without_extending_on_rejection() {
        let mut d = Debounce::default();
        let now = Instant::now();
        assert!(d.accept(now));
        assert!(!d.accept(now + Duration::from_millis(1199)));
        assert!(d.accept(now + Duration::from_millis(1200)));
        assert!(!d.accept(now + Duration::from_millis(1201)));
        assert!(d.accept(now + Duration::from_millis(2400)));
    }
    #[test]
    fn target_toasts_skip_initial_lock_and_repeats_but_include_category_and_new_game() {
        let mut last = None;
        assert!(!target_toast(&mut last, (false, "Desktop".into()), false));
        assert!(!target_toast(&mut last, (false, "Desktop".into()), false));
        assert!(target_toast(&mut last, (true, "Game A".into()), false));
        // alt-tab out and back while Game A runs: silent
        assert!(!target_toast(&mut last, (false, "Desktop".into()), true));
        assert!(!target_toast(&mut last, (true, "Game A".into()), false));
        assert!(target_toast(&mut last, (true, "Game B".into()), false));
        // Game B closed: desktop is news
        assert!(target_toast(&mut last, (false, "Desktop".into()), false));
        last = None;
        assert!(!target_toast(&mut last, (true, "Game B".into()), false));
    }
}
