//! Clip editor page logic (ViewModels/EditorViewModel.cs, Views/EditorView.axaml.cs, Controls/TrimRangeBar.cs).
//! Spec: docs/spec/editor.md.
//!
//! Modules: `model` (the edit as data), `geometry`, `preview` (mpv look), `export` (FFmpeg), `import`,
//! `worker` (background threads), `runtime` (UI-thread state), `bind_*` (button wiring by area), `ui_loop`,
//! `merge` (joining clips).

use crate::app_ctx::AppCtx;
use crate::{AppWindow, EditorVM};
use lag_player::MpvPlayer;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

mod bind_export;
mod bind_frame;
mod bind_montage;
mod bind_navigation;
mod bind_text;
mod bind_timeline;
mod export;
mod geometry;
mod import;
mod merge;
mod model;
mod montage;
mod montage_export;
mod montage_runtime;
mod native;
mod preview;
mod runtime;
#[cfg(test)]
mod tests;
mod ui_loop;
mod worker;

use bind_export::*;
use bind_frame::*;
use bind_montage::*;
use bind_navigation::*;
use bind_text::*;
use bind_timeline::*;
use model::*;
pub(crate) use native::overlay_for_checks;
use runtime::*;
use worker::*;

use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

fn unique_hex_id() -> String {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{:08x}{:016x}{:08x}", std::process::id(), nanos as u64, seq)
}

fn tr(key: &str) -> String {
    lag_core::i18n::tr(key).to_string()
}

fn tr_fmt(key: &str, arg: &str) -> String {
    lag_core::i18n::format(key, &[arg]).unwrap_or_else(|_| tr(key))
}

pub fn init(ui: &AppWindow, ctx: &Rc<AppCtx>) {
    let (commands, rx) = mpsc::channel();
    let (messages_tx, events) = mpsc::channel();
    let tx_w = messages_tx.clone();
    std::thread::Builder::new()
        .name("Lag.EditorMpv".into())
        .spawn(move || mpv_worker(rx, tx_w))
        .expect("editor mpv control worker");

    let mpv_player = Rc::new(RefCell::new(None::<Arc<MpvPlayer>>));

    let native = tr("Option_Native");
    let model = EditModel::new(&native);
    fill_static_options(&ui.global::<EditorVM>(), &model);
    let state = Rc::new(RefCell::new(EditorRuntime::new(ui, ctx, model, commands, messages_tx)));
    state.borrow_mut().sync_clips_from_shared(&ctx.clips());
    bind_app_events(ctx, &state);

    merge::wire(ui, &state);
    bind_navigation(ui, &state);
    bind_timeline(ui, &state);
    bind_frame(ui, &state);
    bind_text(ui, &state);
    bind_export(ui, &state);
    bind_montage(ui, &state);
    ui_loop::run(ui, &state, events, mpv_player);
}
