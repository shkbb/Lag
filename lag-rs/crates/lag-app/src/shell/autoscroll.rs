//! Middle-button autoscroll in every list, as in a browser (owner, 2026-11-21: "hold the wheel and scroll fast in
//! Settings"). Hold the wheel and move: the list under the press point scrolls, faster the further the pointer
//! goes; let go to stop. A short click without moving keeps it on ("free" mode, like Chrome) until the next click.
//!
//! The middle button is taken at window level (winit), before Slint, so a button under the cursor never gets it;
//! the press point and the pointer go to the `AutoScroll` global, and the `LagScrollView` under the press point
//! claims it and scrolls itself (components/scrollbar.slint). Pure state machine here, testable without a window.
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
        WindowEvent::Focused(false) | WindowEvent::CursorLeft { .. } => Input::Lost,
        _ => return EventResult::Propagate,
    };
    let (verdict, ui) = state.step(input, Instant::now(), || window.global::<AutoScroll>().get_owner_area() > 0.0);
    apply(window, ui);
    match verdict {
        Verdict::Pass => EventResult::Propagate,
        Verdict::Swallow => EventResult::PreventDefault,
    }
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
