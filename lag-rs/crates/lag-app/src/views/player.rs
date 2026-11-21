//! Mirrors ViewModels/PlayerViewModel.cs, Views/PlayerView.axaml.cs and Services/VlcEngine.cs.
//! Player v2 (`docs/PLAYER_DESIGN.md`): native child HWND (`wid`) + owned non-activating transparent
//! Slint overlay (`ui/player_overlay.slint`).
//!
//! Modules: `globals` (UI-thread state shared with other pages: cursor hiding, the area covered by floating
//! UI, the fullscreen fade), `worker` (the mpv control thread and its commands), `session` (playback state),
//! `sidebar` (clip list rows), `host` (native video window + overlay window, keyboard focus).
//! This file wires the page (`init`).

use crate::services::AppCtxPlaybackExt;
use crate::shell::nofocus;
use crate::{app_ctx::AppCtx, AppWindow, MainVM, Page, PlayerOverlayVM, PlayerVM, Tr};
use crate::ComponentHandle;
use slint::Model;
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{POINT, RECT},
    UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect},
};

mod bind;
mod globals;
mod host;
mod page;
mod session;
mod sidebar;
#[cfg(test)]
mod tests;
mod tick;
mod worker;

use bind::*;
use globals::*;
pub(crate) use globals::{mark_scripted_input, occluder_rect, toggle_fullscreen_with_fade};
use host::*;
use page::*;
use session::*;
use sidebar::*;
use worker::*;

/// Initialize native surface host, typed event delivery, transport, sidebar and shared editor hook dispatcher.
pub fn init(ui: &AppWindow, ctx: &Rc<AppCtx>) {
    let (page, events) = PlayerPage::new(ui, ctx);
    // Attempt creating native host on the first event loop turn (no-op in headless `--snapshot`).
    {
        let ensure = page.ensure_native_host.clone();
        slint::Timer::single_shot(Duration::ZERO, move || ensure());
    }
    bind_library_events(ui, ctx, &page);
    bind_transport(ui, ctx, &page);
    bind_view(ui, &page);
    tick::run(ui, page, events);
}
