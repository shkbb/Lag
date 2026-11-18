//! Background services (recorder = lag_capture engine adapter, ...). Owned by the shell package.
pub mod clip_merge;
pub mod ffmpeg;
pub mod recorder;
pub mod screenshot;
pub mod sound;

// These files belong to shell-A; shell/mod.rs is owned by shell-B. Export them here until shell-B
// declares/re-exports them itself. No duplicate service instances: ToastService is UI-thread local.
#[path = "../shell/nofocus.rs"]
pub mod nofocus;
#[path = "../shell/toast.rs"]
pub mod toast;

use crate::app_ctx::AppCtx;
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
type PlaybackListener = (Weak<AppCtx>, Rc<dyn Fn(bool)>);
thread_local! { static PLAYBACK: RefCell<Vec<PlaybackListener>> = RefCell::default(); }

/// AppCtx extension while the shared context contract has no playback-stop event.
/// TODO(player/editor packages): subscribe through ctx.on_stop_playback; false stops Player,
/// true stops both Player and Editor (hide-to-tray). Never perform mpv calls on the UI thread.
pub trait AppCtxPlaybackExt {
    #[allow(dead_code)]
    fn on_stop_playback(self: &Rc<Self>, listener: impl Fn(bool) + 'static);
    fn stop_playback(&self, include_editor: bool);
}
impl AppCtxPlaybackExt for AppCtx {
    fn on_stop_playback(self: &Rc<Self>, listener: impl Fn(bool) + 'static) {
        PLAYBACK.with(|listeners| listeners.borrow_mut().push((Rc::downgrade(self), Rc::new(listener))));
    }
    fn stop_playback(&self, include_editor: bool) {
        let callbacks: Vec<_> = PLAYBACK.with(|listeners| {
            let mut listeners = listeners.borrow_mut();
            listeners.retain(|(ctx, _)| ctx.strong_count() > 0);
            listeners.iter().filter(|(ctx, _)| std::ptr::eq(ctx.as_ptr(), self)).map(|(_, f)| f.clone()).collect()
        });
        for f in callbacks {
            f(include_editor);
        }
    }
}
