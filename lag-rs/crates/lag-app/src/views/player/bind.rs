//! The player page's controls: library events, transport, view (rect, fullscreen, pointer, fade).

use super::*;

/// Clip list, settings (theme, language), play / edit requests from other pages.
pub(super) fn bind_library_events(ui: &AppWindow, ctx: &Rc<AppCtx>, page: &PlayerPage) {
    let PlayerPage { host, load, .. } = page;
    sync_sidebar_from_shared(ui, None, &ctx.clips());
    {
        let weak = ui.as_weak();
        let host = host.clone();
        ctx.on_clips_changed(move |clips| {
            if let Some(ui) = weak.upgrade() {
                let h = host.borrow();
                sync_sidebar_from_shared(&ui, h.as_ref().map(|x| &x.overlay), clips);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let ctx_weak = Rc::downgrade(ctx);
        let host = host.clone();
        ctx.on_settings_changed(move |s| {
            if let (Some(ui), Some(ctx)) = (weak.upgrade(), ctx_weak.upgrade()) {
                let (seconds, _) = sidebar_totals(&ctx.clips());
                let dur = format_total_duration(seconds);
                ui.global::<PlayerVM>().set_total_duration(dur.clone().into());
                if let Some(h) = host.borrow().as_ref() {
                    h.overlay.set_theme_mode(s.theme.as_deref().unwrap_or("dark").into());
                    h.overlay.global::<Tr>().set_rev(ui.global::<Tr>().get_rev());
                    h.overlay.global::<PlayerOverlayVM>().set_total_duration(dur.into());
                }
            }
        });
    }
    {
        let load = load.clone();
        ctx.on_play_clip(move |p| load(p));
    }
    {
        let load = load.clone();
        ui.global::<PlayerVM>().on_play_clip(move |p| load(&p));
    }
    {
        let ctx = Rc::downgrade(ctx);
        ui.global::<PlayerVM>().on_edit(move |p| {
            if let Some(ctx) = ctx.upgrade() {
                if !p.is_empty() {
                    ctx.edit_clip(&p);
                }
            }
        });
    }
}

/// Next, play/pause, stop, seek, skip, volume, mute, speed, audio track, page (in)active.
pub(super) fn bind_transport(ui: &AppWindow, ctx: &Rc<AppCtx>, page: &PlayerPage) {
    let PlayerPage { commands, latest_gen, session, host, sync_now, ensure_native_host, load, .. } = page;
    let weak = ui.as_weak();
    {
        let weak = weak.clone();
        let load = load.clone();
        ui.global::<PlayerVM>().on_next(move || {
            let Some(ui) = weak.upgrade() else { return };
            let vm = ui.global::<PlayerVM>();
            let clips = vm.get_clips();
            let n = clips.row_count();
            if n == 0 {
                return;
            }
            let start = clips.iter().position(|c| c.path == vm.get_path()).unwrap_or(n - 1);
            for step in 1..=n {
                if let Some(c) = clips.row_data((start + step) % n) {
                    if !c.is_image {
                        load(&c.path);
                        break;
                    }
                }
            }
        });
    }

    {
        let tx = commands.clone();
        let session = session.clone();
        let load = load.clone();
        ui.global::<PlayerVM>().on_toggle(move || {
            let (stopped, path) = {
                let s = session.borrow();
                (s.stopped && s.has_clip && !s.is_image, s.path.clone())
            };
            if stopped && !path.is_empty() {
                load(&path); // new generation: its events (first frame, time, pause) are accepted again
            } else {
                let _ = tx.send(Command::Toggle);
            }
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_stop(move || {
            let gen = session.borrow_mut().begin_stop();
            latest_gen.store(gen, Ordering::Release);
            let _ = tx.send(Command::Stop);
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<PlayerVM>();
                vm.set_is_playing(false);
                vm.set_is_loading(false);
                vm.set_position(0.0);
                vm.set_time_text(if vm.get_is_image() { "" } else { "0:00.000 / 0:00.000" }.into());
            }
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        ui.global::<PlayerVM>().on_seek(move |v, exact| {
            let _ = tx.send(Command::Seek(v as f64, exact));
        });
    }
    {
        let tx = commands.clone();
        ui.global::<PlayerVM>().on_skip(move |v| {
            let _ = tx.send(Command::Skip(v as f64));
        });
    }
    {
        let tx = commands.clone();
        ui.global::<PlayerVM>().on_volume_changed(move |v| {
            let _ = tx.send(Command::Volume(v as f64));
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let host = host.clone();
        ui.global::<PlayerVM>().on_mute(move || {
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<PlayerVM>();
                let muted = !vm.get_muted();
                vm.set_muted(muted);
                vm.set_mute_flash(false);
                vm.set_mute_flash(true);
                if let Some(h) = host.borrow().as_ref() {
                    let ovm = h.overlay.global::<PlayerOverlayVM>();
                    ovm.set_muted(muted);
                    ovm.set_mute_flash(false);
                    ovm.set_mute_flash(true);
                }
                let _ = tx.send(Command::Mute(muted));
            }
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_speed_changed(move |v| {
            session.borrow_mut().speed = v as f64;
            if let Some(ui) = weak.upgrade() {
                ui.global::<PlayerVM>().set_speed(v);
            }
            let _ = tx.send(Command::Speed(v as f64));
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_audio_changed(move |v| {
            session.borrow_mut().audio_track = v;
            if let Some(ui) = weak.upgrade() {
                ui.global::<PlayerVM>().set_audio_track(v);
            }
            let _ = tx.send(Command::Track(v.max(0) as usize));
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let sync_now = sync_now.clone();
        let ensure_native_host = ensure_native_host.clone();
        ui.global::<PlayerVM>().on_active_changed(move |active| {
            if active {
                ensure_native_host();
            } else {
                let gen = session.borrow_mut().begin_stop();
                latest_gen.store(gen, Ordering::Release);
                let _ = tx.send(Command::Stop);
                if let Some(ui) = weak.upgrade() {
                    let vm = ui.global::<PlayerVM>();
                    vm.set_is_playing(false);
                    vm.set_is_loading(false);
                }
            }
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let sync_now = sync_now.clone();
        ctx.on_stop_playback(move |_| {
            let gen = session.borrow_mut().begin_stop();
            latest_gen.store(gen, Ordering::Release);
            let _ = tx.send(Command::Stop);
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<PlayerVM>();
                vm.set_is_playing(false);
                vm.set_is_loading(false);
            }
            sync_now();
        });
    }
}

/// Video rect, fullscreen switch (+ fade), pointer over the video, floating UI over it.
pub(super) fn bind_view(ui: &AppWindow, page: &PlayerPage) {
    let PlayerPage {
        commands,
        host,
        logical_rect,
        fs_transition,
        overlay_pinned,
        fs_timer,
        fade_timer,
        fade_in_pending,
        cover_applied_at,
        sync_now,
        ..
    } = page;
    let weak = ui.as_weak();
    {
        let logical_rect = logical_rect.clone();
        let fs_transition = fs_transition.clone();
        let fs_timer = fs_timer.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_video_rect_changed(move |x, y, w, h| {
            logical_rect.set((x, y, w, h));
            // During a fullscreen switch the window resizes in several steps; showing the video on the first
            // one let DWM stretch mpv's old-size frame (visible warp, owner 2026-11-16). Keep it hidden until
            // the size has been stable for 150 ms, then show it once at the final size.
            if fs_transition.get() {
                let fs_transition = fs_transition.clone();
                let sync_now = sync_now.clone();
                fs_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(150), move || {
                    fs_transition.set(false);
                    sync_now();
                });
            }
            sync_now();
        });
    }
    {
        let fs_transition = fs_transition.clone();
        let fs_timer = fs_timer.clone();
        let sync_now = sync_now.clone();
        let stats_tx = commands.clone();
        ui.global::<PlayerVM>().on_fullscreen_changed(move |full| {
            log::info!("[Player] fullscreen {}", if full { "on" } else { "off" });
            let _ = stats_tx.send(Command::Stats);
            fs_transition.set(true);
            sync_now();
            let fs_transition = fs_transition.clone();
            let sync_now = sync_now.clone();
            fs_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(250), move || {
                fs_transition.set(false);
                sync_now();
            });
        });
    }
    {
        let weak = weak.clone();
        let host = host.clone();
        let overlay_pinned = overlay_pinned.clone();
        ui.global::<PlayerVM>().on_pointer_left_video(move || {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.global::<MainVM>().get_is_fullscreen() && !overlay_pinned.get() {
                if let Some(h) = host.borrow().as_ref() {
                    h.overlay.global::<PlayerOverlayVM>().set_controls(false);
                }
            }
        });
    }

    ui.global::<PlayerVM>().on_synthetic_move(synthetic_pointer_move);
    {
        // fullscreen: movement over the video re-shows the overlay where its controls live (bottom bar, right list)
        let weak = weak.clone();
        let host = host.clone();
        let logical_rect = logical_rect.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_video_pointer(move |x, y| {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.global::<MainVM>().get_is_fullscreen() {
                return;
            }
            let (_, _, w, hgt) = logical_rect.get();
            let in_bottom = y >= hgt - 140.0;
            let in_right = x >= w - 280.0 && !in_bottom;
            if let Some(h) = host.borrow().as_ref() {
                let paused = !ui.global::<PlayerVM>().get_is_playing();
                if in_bottom || in_right || paused {
                    let ovm = h.overlay.global::<PlayerOverlayVM>();
                    ovm.set_controls(in_bottom);
                    ovm.set_sidebar(in_right);
                    h.overlay_suppressed.set(false);
                }
            }
            sync_now();
        });
    }
    {
        let sync_now = sync_now.clone();
        ui.global::<MainVM>().on_occluder_changed(move |visible, x, y, w, h| {
            OCCLUDER.with(|o| o.set(visible.then_some((x, y, w, h))));
            sync_now();
        });
    }
    // Fullscreen switch behind a full-monitor black cover (shell::fade_cover): fade in ~120 ms, switch, the
    // per-tick check below fades it out once the window and the video have settled.
    {
        let fade_timer_c = fade_timer.clone();
        let fade_in_pending = fade_in_pending.clone();
        let applied_at = cover_applied_at.clone();
        let weak = ui.as_weak();
        let fade: FullscreenFade = Rc::new(move |apply: Box<dyn FnOnce()>| {
            let main = weak.upgrade().and_then(|ui| {
                if ui.window().is_visible() && !ui.window().is_minimized() {
                    nofocus::hwnd(ui.window())
                } else {
                    None
                }
            });
            let Some(main) = main else {
                apply();
                return;
            };
            if fade_in_pending.get() {
                return; // a switch is already running
            }
            crate::shell::fade_cover::show(main, 0);
            let step = Cell::new(0i64);
            let apply = RefCell::new(Some(apply));
            let pending = fade_in_pending.clone();
            let applied_at = applied_at.clone();
            let timer = fade_timer_c.clone();
            fade_timer_c.start(slint::TimerMode::Repeated, Duration::from_millis(15), move || {
                let n = step.get() + 1;
                step.set(n);
                crate::shell::fade_cover::set_alpha((255 * n.min(FADE_OUT_STEPS) / FADE_OUT_STEPS) as u8);
                // one extra tick so the black cover is on screen before the window changes
                if n > FADE_OUT_STEPS {
                    timer.stop();
                    pending.set(true);
                    applied_at.set(Some(Instant::now()));
                    if let Some(a) = apply.borrow_mut().take() {
                        a();
                    }
                }
            });
        });
        FULLSCREEN_FADE.with(|slot| *slot.borrow_mut() = Some(fade));
    }
}
