//! Middle-button autoscroll in every list, as in a browser (owner, 2026-11-21: "hold the wheel and scroll fast in
//! Settings"). Hold the wheel and move: the list under the press point scrolls, faster the further the pointer
//! goes; let go to stop. A short click without moving keeps it on ("free" mode, like Chrome) until the next click.
//!
//! The middle button is taken at window level (winit), before Slint, so a button under the cursor never gets it;
//! the press point goes to the `AutoScroll` global and the `LagScrollView` under it claims it
//! (components/scrollbar.slint). The motion is driven HERE, once per rendered frame (rendering notifier): the
//! pointer is read from Windows itself (so it keeps working outside the window; 2.1.1 stopped there), the distance
//! for this frame = speed x the real time since the last frame, added to `AutoScroll.pending`, which the list
//! consumes. 2.1.0 / 2.1.1 stepped inside the UI on a timer / the animation clock and looked "torn" (owner).
//! Each run logs its frame timing ("[AutoScroll] ...") so a slow renderer shows up in the log.
use crate::{AppWindow, AutoScroll};
use slint::winit_030::{winit, EventResult};
use slint::ComponentHandle;
use std::cell::Cell;
use std::time::{Duration, Instant};
use winit::event::{ElementState, MouseButton, WindowEvent};

/// Moving this far (logical px) while held means "drag-scroll", not a click.
const MOVE_SLOP: f32 = 6.0;
/// A press released sooner than this without moving is a click: free mode.
const CLICK_TIME: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Off,
    Held { at: Instant, moved: bool },
    Free,
}

/// What the window should see of the current event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Swallow,
}

/// What to tell the UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ui {
    Nothing,
    Start(f32, f32),
    Move(f32, f32),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Moved(f32, f32),
    MiddleDown,
    MiddleUp,
    /// Any other button pressed.
    OtherDown,
    Wheel,
    Escape,
    Lost,
}

pub struct State {
    mode: Cell<Mode>,
    pointer: Cell<(f32, f32)>,
    origin: Cell<(f32, f32)>,
}

impl Default for State {
    fn default() -> Self {
        Self { mode: Cell::new(Mode::Off), pointer: Cell::new((0.0, 0.0)), origin: Cell::new((0.0, 0.0)) }
    }
}

impl State {
    #[cfg(test)]
    fn active(&self) -> bool {
        self.mode.get() != Mode::Off
    }

    /// One input; `claimed` = a list under the press point took it (asked only when deciding on free mode).
    pub fn step(&self, input: Input, now: Instant, claimed: impl FnOnce() -> bool) -> (Verdict, Ui) {
        let mode = self.mode.get();
        match input {
            Input::Moved(x, y) => {
                self.pointer.set((x, y));
                match mode {
                    Mode::Off => (Verdict::Pass, Ui::Nothing),
                    Mode::Held { at, moved } => {
                        let (ox, oy) = self.origin.get();
                        let far = (x - ox).hypot(y - oy) > MOVE_SLOP;
                        self.mode.set(Mode::Held { at, moved: moved || far });
                        (Verdict::Pass, Ui::Move(x, y))
                    }
                    Mode::Free => (Verdict::Pass, Ui::Move(x, y)),
                }
            }
            Input::MiddleDown => match mode {
                Mode::Off => {
                    let (x, y) = self.pointer.get();
                    self.origin.set((x, y));
                    self.mode.set(Mode::Held { at: now, moved: false });
                    (Verdict::Swallow, Ui::Start(x, y))
                }
                // a second middle click ends free mode
                _ => self.stop(),
            },
            Input::MiddleUp => match mode {
                Mode::Held { at, moved } if !moved && now.duration_since(at) < CLICK_TIME && claimed() => {
                    self.mode.set(Mode::Free);
                    (Verdict::Swallow, Ui::Nothing)
                }
                Mode::Held { .. } => self.stop(),
                // the release of the click that ended free mode
                _ => (Verdict::Swallow, Ui::Nothing),
            },
            // a click ends free mode and is swallowed, as in a browser
            Input::OtherDown if mode == Mode::Free => self.stop(),
            Input::OtherDown => (Verdict::Pass, Ui::Nothing),
            Input::Escape if mode != Mode::Off => self.stop(),
            Input::Wheel | Input::Lost if mode != Mode::Off => {
                self.mode.set(Mode::Off);
                (Verdict::Pass, Ui::Stop)
            }
            _ => (Verdict::Pass, Ui::Nothing),
        }
    }

    fn stop(&self) -> (Verdict, Ui) {
        self.mode.set(Mode::Off);
        (Verdict::Swallow, Ui::Stop)
    }
}

/// Applies `ui` to the window's `AutoScroll` global (x, y in logical px of the window).
pub fn apply(window: &AppWindow, ui: Ui) {
    let g = window.global::<AutoScroll>();
    match ui {
        Ui::Nothing => {}
        Ui::Start(x, y) => {
            // nobody owns it yet; the views under the point claim it when `active` turns on
            g.set_owner_area(0.0);
            g.set_origin_x(x);
            g.set_origin_y(y);
            g.set_mouse_x(x);
            g.set_mouse_y(y);
            g.set_pending(0.0);
            g.set_active(true);
        }
        Ui::Move(x, y) => {
            g.set_mouse_x(x);
            g.set_mouse_y(y);
        }
        Ui::Stop => g.set_active(false),
    }
}

/// The window-level hook: call first in the window's winit event filter.
pub fn on_event(window: &AppWindow, state: &State, event: &WindowEvent) -> EventResult {
    let scale = window.window().scale_factor().max(0.01);
    let input = match event {
        WindowEvent::CursorMoved { position, .. } => Input::Moved(position.x as f32 / scale, position.y as f32 / scale),
        WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Middle, .. } => Input::MiddleDown,
        WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Middle, .. } => Input::MiddleUp,
        WindowEvent::MouseInput { state: ElementState::Pressed, .. } => Input::OtherDown,
        WindowEvent::MouseWheel { .. } => Input::Wheel,
        WindowEvent::KeyboardInput { event, .. }
            if event.state == ElementState::Pressed
                && event.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) =>
        {
            Input::Escape
        }
        // leaving the window does NOT stop it (owner, 2.1.1: it stopped when the cursor left Lag)
        WindowEvent::Focused(false) => Input::Lost,
        _ => return EventResult::Propagate,
    };
    let (verdict, ui) = state.step(input, Instant::now(), || window.global::<AutoScroll>().get_owner_area() > 0.0);
    apply(window, ui);
    match verdict {
        Verdict::Pass => EventResult::Propagate,
        Verdict::Swallow => EventResult::PreventDefault,
    }
}

/// Scroll speed (px per ms) for a pointer `off` px below (+) / above (-) the press point: a 10 px dead zone, gentle
/// near it, fast further away (~800 px/s at 100 px, ~5000 px/s at 400 px).
pub fn speed(off: f32) -> f32 {
    let d = (off.abs() - 10.0).max(0.0);
    (d / 10.0 + d * d / 4000.0) / 16.0 * off.signum()
}

/// Frame timing of one autoscroll run, for the log.
#[derive(Default)]
struct Frames {
    last: Option<Instant>,
    gaps_ms: Vec<f32>,
}

/// Drives the scrolling from the main window's rendering: call once at start-up.
pub fn install(ui: &AppWindow, state: std::rc::Rc<State>) {
    let frames = std::cell::RefCell::new(Frames::default());
    let weak = ui.as_weak();
    let result = ui.window().set_rendering_notifier(move |rendering, _| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<AutoScroll>();
        let mut f = frames.borrow_mut();
        match rendering {
            slint::RenderingState::BeforeRendering if g.get_active() => {
                // the pointer from Windows: valid outside the window too
                if let Some((x, y)) = cursor_in_window(&ui) {
                    state.pointer.set((x, y));
                    g.set_mouse_x(x);
                    g.set_mouse_y(y);
                }
                let now = Instant::now();
                if let Some(last) = f.last {
                    let dt = now.duration_since(last).as_secs_f32() * 1000.0;
                    f.gaps_ms.push(dt);
                    let (_, oy) = state.origin.get();
                    let step = speed(state.pointer.get().1 - oy) * dt.min(100.0);
                    if step != 0.0 && g.get_owner_area() > 0.0 {
                        g.set_pending(g.get_pending() + step);
                    }
                }
                f.last = Some(now);
            }
            // keep frames coming while it runs (a still pointer outside the window sends no events)
            slint::RenderingState::AfterRendering if g.get_active() => ui.window().request_redraw(),
            slint::RenderingState::BeforeRendering | slint::RenderingState::AfterRendering if f.last.is_some() => {
                log_run(&f.gaps_ms);
                *f = Frames::default();
            }
            _ => {}
        }
    });
    if let Err(e) = result {
        log::warn!("[AutoScroll] no rendering notifier: {e:?}");
    }
}

fn log_run(gaps: &[f32]) {
    if gaps.len() < 2 {
        return;
    }
    let mut sorted = gaps.to_vec();
    sorted.sort_by(f32::total_cmp);
    let avg = gaps.iter().sum::<f32>() / gaps.len() as f32;
    let p95 = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let long = gaps.iter().filter(|g| **g > avg * 1.8).count();
    log::info!(
        "[AutoScroll] {} frames, avg {avg:.1} ms ({:.0} fps), p95 {p95:.1} ms, max {:.1} ms, {long} long gaps",
        gaps.len(),
        1000.0 / avg.max(0.01),
        sorted[sorted.len() - 1]
    );
}

/// The cursor in the window's logical coordinates (may be outside it).
fn cursor_in_window(ui: &AppWindow) -> Option<(f32, f32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let hwnd = crate::shell::nofocus::hwnd(ui.window())?;
    let mut p = POINT::default();
    unsafe {
        GetCursorPos(&mut p).ok()?;
        if !ScreenToClient(hwnd, &mut p).as_bool() {
            return None;
        }
    }
    let scale = ui.window().scale_factor().max(0.01);
    Some((p.x as f32 / scale, p.y as f32 / scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64, t0: Instant) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn hold_and_drag_scrolls_until_release() {
        let (s, t0) = (State::default(), Instant::now());
        assert_eq!(s.step(Input::Moved(100.0, 200.0), t0, || true), (Verdict::Pass, Ui::Nothing));
        assert_eq!(s.step(Input::MiddleDown, t0, || true), (Verdict::Swallow, Ui::Start(100.0, 200.0)));
        assert_eq!(s.step(Input::Moved(100.0, 320.0), at(50, t0), || true), (Verdict::Pass, Ui::Move(100.0, 320.0)));
        // released quickly but it moved: a drag, so it stops
        assert_eq!(s.step(Input::MiddleUp, at(100, t0), || true), (Verdict::Swallow, Ui::Stop));
        assert!(!s.active());
    }

    #[test]
    fn a_long_still_hold_stops_on_release() {
        let (s, t0) = (State::default(), Instant::now());
        s.step(Input::MiddleDown, t0, || true);
        assert_eq!(s.step(Input::MiddleUp, at(800, t0), || true), (Verdict::Swallow, Ui::Stop));
    }

    #[test]
    fn a_click_keeps_it_on_until_the_next_click() {
        let (s, t0) = (State::default(), Instant::now());
        s.step(Input::Moved(10.0, 10.0), t0, || true);
        s.step(Input::MiddleDown, t0, || true);
        // a small tremble is still a click
        s.step(Input::Moved(12.0, 13.0), at(40, t0), || true);
        assert_eq!(s.step(Input::MiddleUp, at(120, t0), || true), (Verdict::Swallow, Ui::Nothing));
        assert!(s.active(), "free mode");
        assert_eq!(s.step(Input::Moved(10.0, 400.0), at(500, t0), || true), (Verdict::Pass, Ui::Move(10.0, 400.0)));
        // a left click ends it and is swallowed (it must not press what is under the pointer)
        assert_eq!(s.step(Input::OtherDown, at(900, t0), || true), (Verdict::Swallow, Ui::Stop));
        assert!(!s.active());
        // the next left click goes through again
        assert_eq!(s.step(Input::OtherDown, at(1000, t0), || true), (Verdict::Pass, Ui::Nothing));
    }

    #[test]
    fn a_click_where_nothing_scrolls_does_not_stay_on() {
        let (s, t0) = (State::default(), Instant::now());
        s.step(Input::MiddleDown, t0, || false);
        assert_eq!(s.step(Input::MiddleUp, at(100, t0), || false), (Verdict::Swallow, Ui::Stop));
        assert_eq!(s.step(Input::OtherDown, at(200, t0), || false), (Verdict::Pass, Ui::Nothing), "clicks are not eaten");
    }

    #[test]
    fn speed_has_a_dead_zone_and_grows_with_distance() {
        assert_eq!(speed(8.0), 0.0);
        assert_eq!(speed(-8.0), 0.0);
        assert!(speed(110.0) > 0.0 && speed(-110.0) < 0.0, "down / up");
        assert!((speed(110.0) * 1000.0 - 781.0).abs() < 5.0, "~800 px/s at 100 px out");
        assert!(speed(410.0) > 4.0 * speed(110.0));
    }

    #[test]
    fn wheel_escape_and_focus_loss_end_it() {
        let t0 = Instant::now();
        for (input, verdict) in [(Input::Wheel, Verdict::Pass), (Input::Escape, Verdict::Swallow), (Input::Lost, Verdict::Pass)] {
            let s = State::default();
            s.step(Input::MiddleDown, t0, || true);
            s.step(Input::MiddleUp, at(100, t0), || true);
            assert!(s.active());
            assert_eq!(s.step(input, at(200, t0), || true), (verdict, Ui::Stop), "{input:?}");
            assert!(!s.active());
        }
        // and while off they are left alone
        let s = State::default();
        assert_eq!(s.step(Input::Escape, t0, || true), (Verdict::Pass, Ui::Nothing));
    }
}
