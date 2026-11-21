//! Mirrors Services/ToastService.cs: sequential six-pending queue, exclusive-fullscreen suppression,
//! 2400ms visible, 220/260ms fade; primary monitor work area, 24 physical-pixel inset.
use super::nofocus;
use crate::ToastWindow;
use crate::ComponentHandle;
use slint::{Timer, TimerMode};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Duration};
use windows::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::*,
};

const MAX_PENDING: usize = 6;
const VISIBLE_MS: u64 = 2400;
const FADE_OUT_MS: u64 = 260;
#[derive(Clone)]
struct Request {
    title: String,
    subtitle: String,
    icon: String,
    accent: String,
}
#[derive(Default)]
struct State {
    queue: VecDeque<Request>,
    current: Option<ToastWindow>,
    theme: String,
    timer: Timer,
}
thread_local! { static STATE: Rc<RefCell<State>> = Rc::new(RefCell::new(State { theme: "dark".into(), ..State::default() })); }

/// UI-thread API. Background callers must marshal via invoke_from_event_loop (as recorder does).
pub fn show(title: String, subtitle: String, icon: &str, accent: &str) {
    STATE.with(|state| {
        {
            let mut s = state.borrow_mut();
            enqueue(&mut s.queue, Request { title, subtitle, icon: icon.into(), accent: accent.into() });
        }
        pump(state);
    });
}
fn enqueue<T>(queue: &mut VecDeque<T>, request: T) {
    if queue.len() >= MAX_PENDING {
        queue.pop_front();
    }
    queue.push_back(request);
}
pub fn set_theme(theme: &str) {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.theme = theme.into();
        if let Some(window) = &s.current {
            window.set_theme_mode(theme.into());
        }
    });
}
fn pump(state: &Rc<RefCell<State>>) {
    loop {
        let request = {
            let mut s = state.borrow_mut();
            if s.current.is_some() {
                return;
            }
            s.queue.pop_front()
        };
        let Some(request) = request else {
            return;
        };
        if nofocus::exclusive_fullscreen() {
            log::info!("[Toast] exclusive-fullscreen app running — sound-only (skipped visual toast).");
            continue;
        }
        let window = match ToastWindow::new() {
            Ok(window) => window,
            Err(e) => {
                log::debug!("[Toast] failed: {e}");
                continue;
            }
        };
        window.set_heading(request.title.into());
        window.set_subtitle(request.subtitle.into());
        window.set_glyph(request.icon.into());
        window.set_accent(request.accent.into());
        window.set_theme_mode(state.borrow().theme.clone().into());
        // Apply BEFORE show; the CBT hook applies styles before first ShowWindow inside winit.
        nofocus::apply_no_focus(window.window(), false, false);
        if let Err(e) = window.show() {
            nofocus::cancel_pending_creation();
            log::debug!("[Toast] failed: {e}");
            continue;
        }
        state.borrow_mut().current = Some(window);
        prepare(state, 0);
        return;
    }
}
fn prepare(state: &Rc<RefCell<State>>, attempts: u32) {
    let ready = {
        let s = state.borrow();
        let Some(window) = &s.current else {
            return;
        };
        if nofocus::hwnd(window.window()).is_some() {
            nofocus::apply_no_focus(window.window(), false, false);
            position(window.window());
            nofocus::show_no_activate(window.window());
            window.set_shown(true);
            true
        } else {
            false
        }
    };
    let weak = Rc::downgrade(state);
    if ready {
        state.borrow().timer.start(TimerMode::SingleShot, Duration::from_millis(VISIBLE_MS), move || {
            if let Some(state) = weak.upgrade() {
                fade_out(&state);
            }
        });
    } else if attempts < 120 {
        state.borrow().timer.start(TimerMode::SingleShot, Duration::from_millis(16), move || {
            if let Some(state) = weak.upgrade() {
                prepare(&state, attempts + 1);
            }
        });
    } else {
        finish(state);
    }
}
fn position(window: &slint::Window) {
    let Some(hwnd) = nofocus::hwnd(window) else {
        return;
    };
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut rect = RECT::default();
        if GetMonitorInfoW(monitor, &mut info).as_bool() && GetWindowRect(hwnd, &mut rect).is_ok() {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                info.rcWork.right - (rect.right - rect.left) - 24,
                info.rcWork.top + 24,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}
fn fade_out(state: &Rc<RefCell<State>>) {
    if let Some(window) = &state.borrow().current {
        window.set_fading_out(true);
        window.set_shown(false);
    }
    let weak = Rc::downgrade(state);
    state.borrow().timer.start(TimerMode::SingleShot, Duration::from_millis(FADE_OUT_MS), move || {
        if let Some(state) = weak.upgrade() {
            finish(&state);
        }
    });
}
fn finish(state: &Rc<RefCell<State>>) {
    nofocus::cancel_pending_creation();
    let old = state.borrow_mut().current.take();
    if let Some(window) = old {
        let _ = window.hide();
    }
    let weak = Rc::downgrade(state);
    Timer::single_shot(Duration::ZERO, move || {
        if let Some(state) = weak.upgrade() {
            pump(&state);
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn burst_drops_oldest_pending_and_preserves_fifo() {
        let mut queue = VecDeque::new();
        for n in 0..10 {
            enqueue(&mut queue, n);
        }
        assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![4, 5, 6, 7, 8, 9]);
        assert_eq!((VISIBLE_MS, FADE_OUT_MS), (2400, 260));
    }
}
