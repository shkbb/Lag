//! The main window keeps the size the user gave it (owner, 2026-11-18: "remember the last window size - after
//! hiding to the tray and after a restart!"). Slint may build a fresh native window when it is shown again from
//! the tray, which came back at the default 1360x840; and nothing persisted the size across launches.
//!
//! The normal (not maximized / fullscreen / minimized) size is tracked from the window's resize events, saved
//! into settings.json a moment after the user stops resizing (`WindowWidth`, `WindowHeight`, `WindowMaximized`,
//! logical px), and applied again before every show. A saved size larger than the current screen is clamped.

use crate::app_ctx::AppCtx;
use crate::AppWindow;
use lag_core::settings::SettingsData;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// The window's own minimum (app.slint `min-width` / `min-height`).
const MIN: (f32, f32) = (1080.0, 640.0);
/// Wait this long after the last resize event before writing settings.json.
const SAVE_DELAY: Duration = Duration::from_millis(800);

thread_local! {
    /// Normal-state size this session (logical px) and whether the window is maximized.
    static SIZE: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
    static MAXIMIZED: Cell<bool> = const { Cell::new(false) };
    static SAVE: RefCell<Option<(slint::Timer, Weak<AppCtx>)>> = const { RefCell::new(None) };
}

/// Reads the saved geometry (call once, before the first show).
pub fn load(settings: &SettingsData) {
    if settings.window_width > 0 && settings.window_height > 0 {
        SIZE.with(|s| s.set(Some((settings.window_width as f32, settings.window_height as f32))));
    }
    MAXIMIZED.with(|m| m.set(settings.window_maximized));
}

/// Where the debounced save goes.
pub fn attach(ctx: &Rc<AppCtx>) {
    SAVE.with(|s| *s.borrow_mut() = Some((slint::Timer::default(), Rc::downgrade(ctx))));
}

/// The saved size, kept inside the screen (a monitor may have been unplugged or rescaled since).
fn clamped(size: (f32, f32), scale: f32) -> (f32, f32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXMAXIMIZED, SM_CYMAXIMIZED};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXMAXIMIZED), GetSystemMetrics(SM_CYMAXIMIZED)) };
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (max_w, max_h) = if sw > 0 && sh > 0 { (sw as f32 / scale, sh as f32 / scale) } else { (f32::MAX, f32::MAX) };
    (size.0.clamp(MIN.0, max_w.max(MIN.0)), size.1.clamp(MIN.1, max_h.max(MIN.1)))
}

/// Before `show()`: give the window its remembered normal size.
pub fn apply_size(ui: &AppWindow) {
    if let Some(size) = SIZE.with(|s| s.get()) {
        let (w, h) = clamped(size, ui.window().scale_factor());
        ui.window().set_size(slint::LogicalSize::new(w, h));
    }
}

/// After `show()`: maximize again when it was maximized.
pub fn apply_state(ui: &AppWindow) {
    if MAXIMIZED.with(|m| m.get()) {
        ui.window().set_maximized(true);
    }
}

/// From the window's resize events (`physical` = the event's new inner size): remember the normal size /
/// maximized state, save a moment later.
pub fn on_resized(ui: &AppWindow, fullscreen: bool, physical: (u32, u32)) {
    let w = ui.window();
    if fullscreen || w.is_minimized() || !w.is_visible() {
        return;
    }
    let maximized = w.is_maximized();
    let changed_state = MAXIMIZED.with(|m| m.replace(maximized)) != maximized;
    let mut changed_size = false;
    if !maximized {
        let size = slint::PhysicalSize::new(physical.0, physical.1).to_logical(w.scale_factor());
        if size.width >= 1.0 && size.height >= 1.0 {
            let new = (size.width.round(), size.height.round());
            changed_size = SIZE.with(|s| s.replace(Some(new))) != Some(new);
        }
    }
    if changed_size || changed_state {
        schedule_save();
    }
}

fn write(ctx: &Rc<AppCtx>) {
    let size = SIZE.with(|s| s.get());
    let maximized = MAXIMIZED.with(|m| m.get());
    ctx.update_settings_silently(|s| {
        if let Some((w, h)) = size {
            s.window_width = w as i32;
            s.window_height = h as i32;
        }
        s.window_maximized = maximized;
    });
}

fn schedule_save() {
    SAVE.with(|s| {
        if let Some((timer, ctx)) = s.borrow().as_ref() {
            let ctx = ctx.clone();
            timer.start(slint::TimerMode::SingleShot, SAVE_DELAY, move || {
                if let Some(ctx) = ctx.upgrade() {
                    write(&ctx);
                }
            });
        }
    });
}

/// On exit: a save still waiting for its delay is written now (before settings.json is flushed).
pub fn save_pending_now() {
    SAVE.with(|s| {
        if let Some((timer, ctx)) = s.borrow().as_ref() {
            if timer.running() {
                timer.stop();
                if let Some(ctx) = ctx.upgrade() {
                    write(&ctx);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_size_stays_inside_limits() {
        // never below the window minimum, never above the screen (this machine's, whatever it is)
        let (w, h) = clamped((10.0, 10.0), 1.0);
        assert_eq!((w, h), MIN);
        let (w, h) = clamped((1_000_000.0, 1_000_000.0), 1.0);
        assert!(w < 1_000_000.0 && h < 1_000_000.0 && w >= MIN.0 && h >= MIN.1);
        let (w, h) = clamped((1500.0, 900.0), 1.0);
        assert!(w <= 1500.0 && h <= 900.0);
    }
}
