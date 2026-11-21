//! Port of `Views/StatsHudWindow.cs`, reusing `lag_capture::overlay::stats::SystemStatsSampler`.
use crate::{app_ctx::AppCtx, shell::nofocus};
use lag_capture::overlay::stats::{Snapshot, SystemStatsSampler};
use lag_core::settings::SettingsData;
use crate::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY},
};

// part of the themed UI now (exported from ui/app.slint): a theme restyles the in-game HUD too
use crate::StatsHud;

static ENGINE_FRAMES: AtomicU64 = AtomicU64::new(0);
thread_local! {static FRAMES: RefCell<Box<dyn Fn() -> u64>> = RefCell::new(Box::new(||ENGINE_FRAMES.load(Ordering::Acquire)));}
/// The engine's frame count from its last status event (used until `set_frame_counter` installs the live one).
pub fn update_frames(frames: u64) {
    ENGINE_FRAMES.store(frames, Ordering::Release);
}
/// The recorder's engine frame counter (set by MainVM), never the sampler's composed-frame FPS.
pub fn set_frame_counter(counter: impl Fn() -> u64 + 'static) {
    FRAMES.with(|f| *f.borrow_mut() = Box::new(counter));
}

pub fn font_size(scale: f64) -> f64 {
    10.0 + scale.clamp(0.03, 0.10) * 140.0
}
pub fn position(bounds: (i32, i32, i32, i32), size: (i32, i32), fraction: (f64, f64)) -> (i32, i32) {
    (
        bounds.0 + (fraction.0 * (bounds.2 - size.0).max(0) as f64) as i32,
        bounds.1 + (fraction.1 * (bounds.3 - size.1).max(0) as f64) as i32,
    )
}
fn fps_text(previous: Option<u64>, frames: u64, elapsed: f64) -> String {
    let Some(last) = previous.filter(|p| frames >= *p) else {
        return "--".into();
    };
    // .NET Math.Round defaults to ties-to-even.
    let fps = ((frames - last) as f64 / elapsed.max(0.2)).round_ties_even() as u64;
    if fps == 0 {
        "--".into()
    } else {
        fps.to_string()
    }
}
fn text(detail: i32, fps: &str, s: Snapshot) -> String {
    match detail.clamp(0, 2) {
        0 => format!("{fps} FPS"),
        1 => format!("{fps} FPS   CPU {}%   GPU {}%", s.cpu_percent, s.gpu_percent),
        _ => format!(
            "{fps} FPS   CPU {}%   GPU {}%   RAM {:.1}/{:.1} GB",
            s.cpu_percent, s.gpu_percent, s.ram_used_gb, s.ram_total_gb
        ),
    }
}
struct Active {
    window: StatsHud,
    sampler: SystemStatsSampler,
    last_frames: Option<u64>,
    last_at: Instant,
    ticks: u32,
}
pub struct Hud {
    active: RefCell<Option<Active>>,
    settings: RefCell<SettingsData>,
    timer: slint::Timer,
}
impl Hud {
    pub fn new(ctx: &Rc<AppCtx>) -> Rc<Self> {
        let hud = Rc::new(Self {
            active: RefCell::new(None),
            settings: RefCell::new(ctx.settings()),
            timer: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&hud);
        ctx.on_settings_changed(move |s| {
            if let Some(h) = weak.upgrade() {
                *h.settings.borrow_mut() = s.clone();
                h.tick();
            }
        });
        let weak = Rc::downgrade(&hud);
        hud.timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            if let Some(h) = weak.upgrade() {
                h.tick();
            }
        });
        // First show is deferred until the native UI message loop is running.
        let weak = Rc::downgrade(&hud);
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(h) = weak.upgrade() {
                h.tick();
            }
        });
        hud
    }
    fn tick(&self) {
        let settings = self.settings.borrow().clone();
        // C# MainViewModel: `OverlayEnabled && StatsShown` (owner, 2026-11-17: turning the overlay off left the
        // stats on screen)
        if !(settings.overlay_enabled && settings.stats_shown) {
            if let Some(active) = self.active.borrow_mut().take() {
                let _ = active.window.hide();
            }
            return;
        }
        // Do not even create an overlay over an exclusive-fullscreen game.
        if nofocus::exclusive_fullscreen() {
            if let Some(active) = self.active.borrow().as_ref() {
                let _ = active.window.hide();
            }
            return;
        }
        let mut slot = self.active.borrow_mut();
        if slot.is_none() {
            match (StatsHud::new(), SystemStatsSampler::new()) {
                (Ok(window), Ok(sampler)) => {
                    *slot = Some(Active { window, sampler, last_frames: None, last_at: Instant::now(), ticks: 0 })
                }
                _ => {
                    log::warn!("[StatsHud] window styling failed: HUD initialization failed");
                    return;
                }
            }
        }
        let active = slot.as_mut().unwrap();
        active.window.set_stats_font_size(font_size(settings.stats_scale) as f32);
        let frames = FRAMES.with(|f| f.borrow()());
        let now = Instant::now();
        let fps = fps_text(active.last_frames, frames, (now - active.last_at).as_secs_f64());
        active.ticks = active.ticks.wrapping_add(1);
        if active.ticks % 10 == 1 {
            log::info!("[StatsHud] engine frames={frames} fps={fps}");
        }
        active.last_frames = Some(frames);
        active.last_at = now;
        active.window.set_stats_text(text(settings.stats_detail, &fps, active.sampler.current()).into());
        let window = active.window.window();
        // Arms shell-A's native creation guard BEFORE the first ShowWindow, and stamps any existing HWND.
        nofocus::apply_no_focus(window, true, true);
        if !window.is_visible() {
            let _ = active.window.show();
        }
        nofocus::apply_no_focus(window, true, true);
        nofocus::show_no_activate(window);
        reposition(window, settings.stats_x, settings.stats_y);
    }
}
fn reposition(window: &slint::Window, xf: f64, yf: f64) {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let b = info.rcMonitor;
            let size = window.size();
            let (x, y) = position(
                (b.left, b.top, b.right - b.left, b.bottom - b.top),
                (size.width as i32, size.height as i32),
                (xf, yf),
            );
            window.set_position(slint::PhysicalPosition::new(x, y));
        }
    }
}
impl Drop for Hud {
    fn drop(&mut self) {
        self.timer.stop();
        if let Some(active) = self.active.get_mut().take() {
            let _ = active.window.hide();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn font_scale_clamps() {
        assert!((font_size(0.045) - 16.3).abs() < 1e-9);
        assert_eq!(font_size(0.), 14.2);
        assert_eq!(font_size(1.), 24.);
    }
    #[test]
    fn screen_fraction_math() {
        assert_eq!(position((0, 0, 1920, 1080), (400, 30), (0.5, 0.02)), (760, 21));
        assert_eq!(position((-1920, -100, 1920, 1080), (400, 30), (1., 1.)), (-400, 950));
        assert_eq!(position((50, 70, 100, 100), (200, 200), (0.5, 0.5)), (50, 70));
        assert_eq!(position((0, 0, 2560, 1440), (600, 60), (0.5, 0.02)), (980, 27));
    }
    #[test]
    fn fps_counter_delta_reset_idle_and_rounding() {
        assert_eq!(fps_text(None, 60, 1.), "--");
        assert_eq!(fps_text(Some(60), 0, 1.), "--");
        assert_eq!(fps_text(Some(60), 60, 1.), "--");
        assert_eq!(fps_text(Some(60), 180, 1.), "120");
        assert_eq!(fps_text(Some(0), 1, 0.01), "5");
        assert_eq!(fps_text(Some(0), 25, 10.), "2");
        let s = Snapshot { cpu_percent: 20, gpu_percent: 30, ram_used_gb: 8., ram_total_gb: 16., ..Default::default() };
        assert_eq!(text(0, "60", s), "60 FPS");
        assert_eq!(text(2, "60", s), "60 FPS   CPU 20%   GPU 30%   RAM 8.0/16.0 GB");
    }
}
