//! The player's 33 ms tick: worker messages, overlay/cursor idle in fullscreen, keyboard focus, fade-in.

use super::*;

pub(super) fn run(ui: &AppWindow, page: PlayerPage, events: mpsc::Receiver<Message>) {
    let PlayerPage {
        commands,
        latest_gen,
        session,
        host,
        logical_rect,
        fs_transition,
        overlay_pinned,
        fade_timer,
        fade_in_pending,
        cover_applied_at,
        ensure_native_host,
        load: _,
        ..
    } = page;
    let weak = ui.as_weak();
    let timer = Rc::new(slint::Timer::default());
    let keep_timer = timer.clone();
    ui.global::<PlayerVM>().on_frame_step({
        let tx = commands.clone();
        move || {
            let _ = &keep_timer;
            let _ = tx.send(Command::Step);
        }
    });

    let mut was_visible = true;
    let last_focus_class = RefCell::new(String::new());
    // fullscreen idle: when the controls went away (their fade-out must finish before the overlay window hides)
    let idle_since: Cell<Option<Instant>> = Cell::new(None);
    // OS cursor position at the last tick and when it last changed (fullscreen cursor idle)
    let cursor_last = Cell::new((i32::MIN, i32::MIN));
    let cursor_moved_at = Cell::new(Instant::now());
    let cursor_proc_on = Cell::new(0isize); // overlay HWND that already has cursor_proc
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
        ensure_native_host();
        let Some(ui) = weak.upgrade() else { return };
        let vm = ui.global::<PlayerVM>();
        let mvm = ui.global::<MainVM>();
        let page_visible =
            ui.window().is_visible() && !ui.window().is_minimized() && mvm.get_current_page() == Page::Player;
        if was_visible && !page_visible {
            let gen = session.borrow_mut().begin_stop();
            latest_gen.store(gen, Ordering::Release);
            let _ = commands.send(Command::Stop);
            vm.set_is_playing(false);
            vm.set_is_loading(false);
        }
        let just_hidden = was_visible && !page_visible;
        was_visible = page_visible;

        for message in events.try_iter() {
            match message {
                Message::LoadStarted(gen) => {
                    session.borrow_mut().on_worker_load_started(gen);
                }
                Message::Event(event_gen, event) => {
                    let dragging = vm.get_dragging();
                    let delta = session.borrow_mut().apply_event(event_gen, &event, dragging);
                    if delta.applied {
                        let s = session.borrow();
                        vm.set_is_loading(s.is_loading);
                        vm.set_is_playing(s.is_playing);
                        vm.set_position(s.norm_position);
                        vm.set_audio_track_count(s.audio_track_count);
                        vm.set_error(s.error.as_str().into());
                        if delta.select_default_track {
                            let _ = commands.send(Command::Track(0));
                        }
                    }
                }
                Message::Still(gen, path, pixels) => {
                    let s = session.borrow();
                    if gen == s.load_gen && vm.get_path() == path {
                        if let Some(pixels) = pixels {
                            vm.set_image(pixels.into_image());
                        } else {
                            log::warn!("[Player] Failed to load screenshot: {path}");
                        }
                    }
                }
            }
        }

        // Lift the fullscreen cover once the window has settled and the video (if any) is back on screen.
        if fade_in_pending.get() {
            let since = cover_applied_at.get().map(|t| t.elapsed()).unwrap_or_default();
            let video_back = host.borrow().as_ref().is_some_and(|h| h.surface.is_visible())
                || !vm.get_has_clip()
                || vm.get_is_image()
                || !page_visible;
            let settled = !fs_transition.get() && since >= Duration::from_millis(140);
            if (settled && video_back) || since >= Duration::from_millis(1500) {
                fade_in_pending.set(false);
                let step = Cell::new(0i64);
                let timer = fade_timer.clone();
                fade_timer.start(slint::TimerMode::Repeated, Duration::from_millis(15), move || {
                    let n = step.get() + 1;
                    step.set(n);
                    let a = 255 - 255 * n.min(FADE_IN_STEPS) / FADE_IN_STEPS;
                    crate::shell::fade_cover::set_alpha(a as u8);
                    if n >= FADE_IN_STEPS {
                        timer.stop();
                        crate::shell::fade_cover::hide();
                    }
                });
            }
        }
        // Nothing to draw while the player page is not on screen (tray, minimized, other page, game in
        // front): skip the per-tick overlay/property sync, but run it once on the hide transition so the
        // surface and the overlay window actually get hidden.
        if !page_visible && !just_hidden {
            return;
        }
        if !vm.get_is_image() {
            vm.set_time_text(session.borrow().time_display().into());
        }

        if let Some(h) = host.borrow().as_ref() {
            h.check_cursor_auto_hide(mvm.get_is_fullscreen(), overlay_pinned.get());
            keep_keyboard_focus(h.main_hwnd, nofocus::hwnd(h.overlay.window()), &last_focus_class);
            {
                let ovm = h.overlay.global::<PlayerOverlayVM>();
                let idle = mvm.get_is_fullscreen()
                    && session.borrow().is_playing
                    && !ovm.get_controls()
                    && !ovm.get_sidebar()
                    && !overlay_pinned.get();
                if idle {
                    let since = idle_since.get().unwrap_or_else(|| {
                        let now = Instant::now();
                        idle_since.set(Some(now));
                        now
                    });
                    // 300 ms > the 250 ms fade-out of the bar / clip list: hide the window only after it played
                    if since.elapsed() >= Duration::from_millis(300) {
                        if !h.overlay_suppressed.get() {
                            // hidden by inactivity -> keep the cursor hidden over the main window too
                            ui.global::<PlayerVM>().set_cursor_hidden(ovm.get_cursor_idle());
                        }
                        h.overlay_suppressed.set(true);
                    }
                } else {
                    idle_since.set(None);
                    if !mvm.get_is_fullscreen() || !session.borrow().is_playing {
                        h.overlay_suppressed.set(false); // windowed, or paused (the big play button must show)
                    }
                }
                // keep the cursor hidden while idle and playing (see hide_cursor_over). Owner, 2026-11-17:
                // windowed worked, fullscreen did not - there the hide depended on a main-window Slint timer
                // that only a pointer event over the main window started (and the overlay hiding under a
                // still cursor sends none that counts). Idle is now measured here from the OS cursor itself.
                let mut pt = POINT::default();
                if unsafe { GetCursorPos(&mut pt) }.is_ok() && cursor_last.get() != (pt.x, pt.y) {
                    cursor_last.set((pt.x, pt.y));
                    cursor_moved_at.set(Instant::now());
                }
                if session.borrow().is_playing && mvm.get_current_page() == Page::Player {
                    let mut area = RECT::default();
                    let still = cursor_moved_at.get().elapsed() >= Duration::from_millis(2500);
                    let target = if h.overlay_shown.get() && ovm.get_cursor_idle() {
                        nofocus::hwnd(h.overlay.window())
                    } else if mvm.get_is_fullscreen() && still && !overlay_pinned.get() {
                        Some(h.main_hwnd)
                    } else {
                        None
                    };
                    if let Some(ov) = nofocus::hwnd(h.overlay.window()) {
                        if cursor_proc_on.get() != ov.0 as isize {
                            install_cursor_proc(ov);
                            cursor_proc_on.set(ov.0 as isize);
                        }
                    }
                    match target {
                        Some(w) if unsafe { GetWindowRect(w, &mut area) }.is_ok() => hide_cursor_over(area),
                        _ => release_cursor_hide(),
                    }
                } else {
                    release_cursor_hide();
                }
            }
            sync_overlay_from_vm(&ui, &h.overlay, &session.borrow());
            let ready = session.borrow().video_ready;
            h.sync(&ui, logical_rect.get(), fs_transition.get(), ready);
        }
    });
}
