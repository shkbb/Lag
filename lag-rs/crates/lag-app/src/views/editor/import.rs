//! "Import": the system Open dialog for any video file.

use super::tr;
use crate::AppWindow;
use crate::ComponentHandle;
use slint::{Timer, TimerMode};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

thread_local! {
    /// One Open dialog at a time.
    static DIALOG_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Shows the Open dialog (modal to the main window) on its own thread - it pumps its own messages while Slint keeps
/// rendering - and calls `done` with the picked file on the UI thread. Nothing happens when it is cancelled.
pub(super) fn ask_for_video(ui: &AppWindow, done: impl FnOnce(String) + 'static) {
    if DIALOG_OPEN.with(|d| d.replace(true)) {
        return;
    }
    let owner = crate::shell::nofocus::hwnd(ui.window()).map_or(0, |h| h.0 as isize);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(pick_video_file(owner));
    });
    let timer = Rc::new(Timer::default());
    let keep = timer.clone();
    let done = std::cell::RefCell::new(Some(done));
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let Ok(picked) = rx.try_recv() else { return };
        keep.stop();
        DIALOG_OPEN.with(|d| d.set(false));
        if let (Some(path), Some(done)) = (picked, done.borrow_mut().take()) {
            done(path);
        }
    });
}

/// Video file types the editor imports (everything FFmpeg/mpv here decodes that people actually have).
pub(super) const IMPORT_PATTERN: &str = "*.mp4;*.mkv;*.mov;*.webm;*.avi;*.m4v;*.wmv;*.flv;*.ts;*.mts;*.m2ts";

/// System "Open" dialog for one video file, modal to `owner` (HWND as isize, 0 = none). None = cancelled.
pub(super) fn pick_video_file(owner: isize) -> Option<String> {
    use windows::{
        core::PCWSTR,
        Win32::{Foundation::HWND, System::Com::*, UI::Shell::Common::COMDLG_FILTERSPEC, UI::Shell::*},
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().ok()?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _com = Apartment;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.SetOptions(FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR).ok()?;
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let title = wide(&tr("Editor_ImportTitle"));
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        let (name, pattern) = (wide(&tr("Editor_ImportFilter")), wide(IMPORT_PATTERN));
        let spec = [COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(pattern.as_ptr()) }];
        let _ = dialog.SetFileTypes(&spec);
        let owner = (owner != 0).then_some(HWND(owner as *mut _));
        dialog.Show(owner).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let result = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast()));
        result
    }
}
