//! Shared application context (UI thread only). Replaces C#'s DI container + cross-view-model wiring
//! (MainViewModel subscribing to Settings/Library/Player events). Every view module gets `Rc<AppCtx>`.
//!
//! Rules: lives on the UI thread (`Rc`, `RefCell`); background threads talk back via
//! `slint::invoke_from_event_loop` and then use the ctx. Never hold a `RefCell` borrow across a call that
//! may re-enter (listeners are invoked after the borrow is released).

use lag_core::settings::SettingsData;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

type Listener<T> = RefCell<Vec<Rc<dyn Fn(&T)>>>;

/// Shared clip record published by Library and consumed by Player/Editor.
pub type SharedClip = lag_core::clip::ReplayClip<slint::SharedPixelBuffer<slint::Rgba8Pixel>>;

pub struct AppCtx {
    settings: RefCell<SettingsData>,
    save_timer: slint::Timer,
    save_pending: Cell<bool>,
    settings_listeners: Listener<SettingsData>,
    library_refresh_listeners: Listener<()>,
    play_clip_listeners: Listener<String>,
    edit_clip_listeners: Listener<String>,
    clips: RefCell<Vec<SharedClip>>,
    clips_listeners: Listener<Vec<SharedClip>>,
}

impl AppCtx {
    pub fn new(settings: SettingsData) -> Rc<Self> {
        Rc::new(Self {
            settings: RefCell::new(settings),
            save_timer: slint::Timer::default(),
            save_pending: Cell::new(false),
            settings_listeners: RefCell::default(),
            library_refresh_listeners: RefCell::default(),
            play_clip_listeners: RefCell::default(),
            edit_clip_listeners: RefCell::default(),
            clips: RefCell::default(),
            clips_listeners: RefCell::default(),
        })
    }

    /// Snapshot of the current settings.
    pub fn settings(&self) -> SettingsData {
        self.settings.borrow().clone()
    }

    /// Mutates the settings, schedules the debounced save (350 ms, C# `_saveDebounce`) and notifies
    /// listeners (after the borrow is released).
    pub fn update_settings(self: &Rc<Self>, f: impl FnOnce(&mut SettingsData)) {
        f(&mut self.settings.borrow_mut());
        self.schedule_save();
        let snapshot = self.settings();
        let listeners: Vec<_> = self.settings_listeners.borrow().clone();
        for l in listeners {
            l(&snapshot);
        }
    }

    /// Like `update_settings`, but nobody is told: for values no listener cares about (the window's size),
    /// so resizing never re-runs theme / hotkey / recorder reactions.
    pub fn update_settings_silently(self: &Rc<Self>, f: impl FnOnce(&mut SettingsData)) {
        f(&mut self.settings.borrow_mut());
        self.schedule_save();
    }

    pub fn on_settings_changed(&self, f: impl Fn(&SettingsData) + 'static) {
        self.settings_listeners.borrow_mut().push(Rc::new(f));
    }

    fn schedule_save(self: &Rc<Self>) {
        self.save_pending.set(true);
        let me = Rc::downgrade(self);
        self.save_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(350), move || {
            if let Some(me) = me.upgrade() {
                me.flush_settings();
            }
        });
    }

    /// Writes settings.json now if a save is pending (C# FlushPendingSave on shutdown).
    pub fn flush_settings(&self) {
        if !self.save_pending.replace(false) {
            return;
        }
        let data = self.settings();
        if let Some(path) = lag_core::settings::default_path() {
            if let Err(e) = data.save(&path) {
                log::error!("[Settings] save failed: {e}");
            }
        }
    }

    pub fn request_library_refresh(&self) {
        let listeners: Vec<_> = self.library_refresh_listeners.borrow().clone();
        for l in listeners {
            l(&());
        }
    }
    pub fn on_library_refresh(&self, f: impl Fn() + 'static) {
        self.library_refresh_listeners.borrow_mut().push(Rc::new(move |_| f()));
    }

    /// Library -> Player (C# Library.PlayClipRequested).
    pub fn play_clip(&self, path: &str) {
        let listeners: Vec<_> = self.play_clip_listeners.borrow().clone();
        for l in listeners {
            l(&path.to_string());
        }
    }
    pub fn on_play_clip(&self, f: impl Fn(&String) + 'static) {
        self.play_clip_listeners.borrow_mut().push(Rc::new(f));
    }

    /// Library/Player -> Editor (C# Library.EditClipRequested / OpenEditorFromPlayer).
    pub fn edit_clip(&self, path: &str) {
        let listeners: Vec<_> = self.edit_clip_listeners.borrow().clone();
        for l in listeners {
            l(&path.to_string());
        }
    }
    pub fn on_edit_clip(&self, f: impl Fn(&String) + 'static) {
        self.edit_clip_listeners.borrow_mut().push(Rc::new(f));
    }

    /// Snapshot of the library's current clip collection (newest first).
    pub fn clips(&self) -> Vec<SharedClip> {
        self.clips.borrow().clone()
    }

    /// Publishes the library's current clip collection and notifies Player/Editor subscribers.
    pub fn set_clips(&self, clips: Vec<SharedClip>) {
        *self.clips.borrow_mut() = clips;
        let snapshot = self.clips();
        let listeners: Vec<_> = self.clips_listeners.borrow().clone();
        for l in listeners {
            l(&snapshot);
        }
    }

    pub fn on_clips_changed(&self, f: impl Fn(&Vec<SharedClip>) + 'static) {
        self.clips_listeners.borrow_mut().push(Rc::new(f));
    }
}
