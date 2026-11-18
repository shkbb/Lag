//! Page view models. Each module owns ONE page (its .slint file under ui/views and its `<Name>VM` global).
//! `main_vm` is the MainViewModel (top bar, navigation, recording control) owned by the shell package.
pub mod editor;
pub mod library;
pub mod main_vm;
pub mod player;
pub mod settings;

use crate::app_ctx::AppCtx;
use std::rc::Rc;

pub fn init_all(ui: &crate::AppWindow, ctx: &Rc<AppCtx>) {
    main_vm::init(ui, ctx);
    settings::init(ui, ctx);
    library::init(ui, ctx);
    player::init(ui, ctx);
    editor::init(ui, ctx);
}
