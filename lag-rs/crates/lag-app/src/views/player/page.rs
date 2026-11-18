//! The player page's shared state: the handles every part of the page (transport, view, tick) works with.

use super::*;

/// Fullscreen fade: black cover fades in over FADE_OUT_STEPS x 15 ms before the switch, out over FADE_IN_STEPS.
pub(super) const FADE_OUT_STEPS: i64 = 8; // ~120 ms
pub(super) const FADE_IN_STEPS: i64 = 12; // ~180 ms

/// Handles shared by the parts of the player page. Built once by `init`.
pub(super) struct PlayerPage {
    pub(super) commands: mpsc::Sender<Command>,
    pub(super) latest_gen: Arc<AtomicU64>,
    pub(super) session: Rc<RefCell<PlayerSessionState>>,
    pub(super) host: Rc<RefCell<Option<NativeHost>>>,
    pub(super) logical_rect: Rc<Cell<(f32, f32, f32, f32)>>,
    pub(super) fs_transition: Rc<Cell<bool>>,
    pub(super) overlay_pinned: Rc<Cell<bool>>,
    pub(super) fs_timer: Rc<slint::Timer>,
    pub(super) fade_timer: Rc<slint::Timer>,
    pub(super) fade_in_pending: Rc<Cell<bool>>,
    pub(super) cover_applied_at: Rc<Cell<Option<Instant>>>,
    pub(super) sync_now: Rc<dyn Fn()>,
    pub(super) ensure_native_host: Rc<dyn Fn()>,
    pub(super) load: Rc<dyn Fn(&str)>,
}

impl PlayerPage {
    /// Starts the mpv control worker and builds the page state; returns the worker's message receiver too.
    pub(super) fn new(ui: &AppWindow, ctx: &Rc<AppCtx>) -> (Self, mpsc::Receiver<Message>) {
        let (commands, rx) = mpsc::channel();
        let (messages, events) = mpsc::channel();
        let latest_gen = Arc::new(AtomicU64::new(0));
        {
            let tx = messages.clone();
            let latest = latest_gen.clone();
            std::thread::Builder::new()
                .name("Lag.MpvControl".into())
                .spawn(move || worker(rx, tx, latest))
                .expect("player control worker");
        }

        let session = Rc::new(RefCell::new(PlayerSessionState::default()));
        let host: Rc<RefCell<Option<NativeHost>>> = Rc::new(RefCell::new(None));
        let logical_rect = Rc::new(Cell::new((0.0f32, 0.0f32, 0.0f32, 0.0f32)));
        let fs_transition = Rc::new(Cell::new(false));
        let overlay_pinned = Rc::new(Cell::new(false));
        let fs_timer = Rc::new(slint::Timer::default());
        // Fullscreen fade: out (0 -> -100 brightness) before the switch, in (-100 -> 0) once the video is shown
        // again at its final size.
        let fade_timer = Rc::new(slint::Timer::default());
        let fade_in_pending = Rc::new(Cell::new(false));
        let cover_applied_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));

        let sync_now: Rc<dyn Fn()> = {
            let weak = ui.as_weak();
            let host = host.clone();
            let logical_rect = logical_rect.clone();
            let fs_transition = fs_transition.clone();
            let session = session.clone();
            Rc::new(move || {
                let Some(ui) = weak.upgrade() else { return };
                if let Ok(h) = host.try_borrow() {
                    if let Some(h) = h.as_ref() {
                        sync_overlay_from_vm(&ui, &h.overlay, &session.borrow());
                        let ready = session.borrow().video_ready;
                        h.sync(&ui, logical_rect.get(), fs_transition.get(), ready);
                    }
                }
            })
        };

        let ensure_native_host: Rc<dyn Fn()> = {
            let weak = ui.as_weak();
            let ctx_weak = Rc::downgrade(ctx);
            let host = host.clone();
            let commands = commands.clone();
            let sync_now = sync_now.clone();
            let overlay_pinned = overlay_pinned.clone();
            Rc::new(move || {
                let (Some(ui), Some(ctx)) = (weak.upgrade(), ctx_weak.upgrade()) else {
                    return;
                };
                if host.borrow().is_some() {
                    return;
                }
                let Some(main_hwnd) = nofocus::hwnd(ui.window()) else {
                    return;
                };
                match NativeHost::new(&ui, main_hwnd, &ctx) {
                    Ok(h) => {
                        let wid = h.surface.wid();
                        let pinned_flag = overlay_pinned.clone();
                        h.overlay.global::<PlayerOverlayVM>().on_pinned_changed(move |p| {
                            pinned_flag.set(p);
                        });
                        *host.borrow_mut() = Some(h);
                        let sync_cb = sync_now.clone();
                        HOST_SYNC.with(|slot| *slot.borrow_mut() = Some(sync_cb));
                        let _ = commands.send(Command::InitNative(wid));
                        // slow-motion blending switch in the speed menu (persisted)
                        {
                            let on = ctx.settings().slowmo_interpolation;
                            h_ref_smooth(&host, on);
                            let _ = commands.send(Command::Smooth(on));
                            let tx = commands.clone();
                            let ctx_w = Rc::downgrade(&ctx);
                            let host_w = Rc::downgrade(&host);
                            if let Some(h) = host.borrow().as_ref() {
                                h.overlay.global::<PlayerOverlayVM>().on_smooth_slowmo_changed(move |v| {
                                    if let Some(ctx) = ctx_w.upgrade() {
                                        ctx.update_settings(|s| s.slowmo_interpolation = v);
                                    }
                                    if let Some(host) = host_w.upgrade() {
                                        h_ref_smooth(&host, v);
                                    }
                                    let _ = tx.send(Command::Smooth(v));
                                });
                            }
                        }
                        sync_now();
                    }
                    Err(e) => {
                        log::error!("[Player] native surface init failed: {e}");
                        ui.global::<PlayerVM>().set_error(e.to_string().into());
                    }
                }
            })
        };

        let weak = ui.as_weak();
        let load: Rc<dyn Fn(&str)> = {
            let commands = commands.clone();
            let weak = weak.clone();
            let session = session.clone();
            let latest_gen = latest_gen.clone();
            let ensure_native_host = ensure_native_host.clone();
            let sync_now = sync_now.clone();
            let host = host.clone();
            Rc::new(move |path| {
                ensure_native_host();
                let Some(ui) = weak.upgrade() else { return };
                let vm = ui.global::<PlayerVM>();
                let gen = session.borrow_mut().begin_load(path);
                latest_gen.store(gen, Ordering::Release);

                let is_img = is_image(path);
                vm.set_path(path.into());
                vm.set_has_clip(true);
                vm.set_is_image(is_img);
                vm.set_is_loading(!is_img);
                vm.set_is_playing(false);
                vm.set_position(0.0);
                vm.set_time_text(if is_img { "" } else { "0:00.000 / 0:00.000" }.into());
                vm.set_speed(1.0);
                vm.set_audio_track(0);
                vm.set_audio_track_count(0);
                vm.set_error("".into());
                vm.set_image(slint::Image::default());

                if let Some(h) = host.borrow().as_ref() {
                    let ovm = h.overlay.global::<PlayerOverlayVM>();
                    ovm.set_position(0.0);
                    ovm.set_dragging(false);
                }

                let clip = vm.get_clips().iter().find(|c| c.path == path);
                vm.set_title(clip.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| {
                    Path::new(path).file_stem().unwrap_or_default().to_string_lossy().to_string().into()
                }));
                vm.set_meta(clip.map(|c| c.header_meta).unwrap_or_default());
                ui.global::<MainVM>().set_current_page(Page::Player);
                let _ = commands.send(Command::Load { path: path.to_owned(), gen });
                sync_now();
            })
        };
        (
            Self {
                commands,
                latest_gen,
                session,
                host,
                logical_rect,
                fs_transition,
                overlay_pinned,
                fs_timer,
                fade_timer,
                fade_in_pending,
                cover_applied_at,
                sync_now,
                ensure_native_host,
                load,
            },
            events,
        )
    }
}
