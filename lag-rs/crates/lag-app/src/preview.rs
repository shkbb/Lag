//! Theme preview with live reload (docs/spec/themes.md, phase 3): `Lag.exe --theme-preview <theme folder>
//! [--at x,y,w,h]`. A separate Lag window drawing the theme over real data (the library, settings...), without
//! recording, tray, hotkeys or the cloud, and never saving settings: safe next to the running Lag. It watches the
//! theme folder; after a save it starts a fresh preview at the same place; the new one takes the old one's place
//! in the window order and then closes it, so there is no gap and the preview never falls behind other windows
//! (Windows does not let a background process come to the front). Every reload is exactly what Lag would show.
//! A theme that does not compile shows its error text instead, and the fix reloads it the same way.
use crate::{app_ctx, startup, themes, ui_twin, views, AppWindow, ComponentHandle, MainVM, ThemeErrorWindow, Tr};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Args {
    pub dir: PathBuf,
    /// where the previous preview was (physical x, y, w, h)
    pub at: Option<[i32; 4]>,
    /// the previous preview to replace: (process id, window handle)
    pub replace: Option<(u32, isize)>,
}

pub fn args() -> Option<Args> {
    let a: Vec<String> = std::env::args().collect();
    let i = a.iter().position(|x| x == "--theme-preview")?;
    let dir = PathBuf::from(a.get(i + 1)?);
    let numbers = |flag: &str| -> Vec<i64> {
        a.iter().position(|x| x == flag).and_then(|j| a.get(j + 1)).map_or(Vec::new(), |v| v.split(',').filter_map(|p| p.parse().ok()).collect())
    };
    let at = numbers("--at");
    let replace = numbers("--replace");
    Some(Args {
        dir,
        at: (at.len() == 4).then(|| [at[0] as i32, at[1] as i32, at[2] as i32, at[3] as i32]),
        replace: (replace.len() == 2).then(|| (replace[0] as u32, replace[1] as isize)),
    })
}

/// What changed in a theme (a `.lag` file, or a folder): every file's path, size and time, summed into one number.
fn fingerprint(dir: &Path) -> u64 {
    if dir.is_file() {
        let m = std::fs::metadata(dir).ok();
        let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
        return t.rotate_left(17) ^ m.map_or(0, |m| m.len()).rotate_left(41);
    }
    let mut h = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
            let name: u64 = p.to_string_lossy().bytes().fold(1469598103934665603u64, |a, b| (a ^ b as u64).wrapping_mul(1099511628211));
            h = h.wrapping_add(name ^ t.rotate_left(17) ^ m.len().rotate_left(41));
        }
    }
    h
}

/// Starts a fresh preview at `at`; it replaces this one (`take_place`). Returns whether it started.
fn relaunch(dir: &Path, at: [i32; 4], hwnd: isize) -> bool {
    let spawned = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("--theme-preview")
            .arg(dir)
            .arg("--at")
            .arg(format!("{},{},{},{}", at[0], at[1], at[2], at[3]))
            .arg("--replace")
            .arg(format!("{},{hwnd}", std::process::id()))
            .spawn()
    });
    if let Err(e) = &spawned {
        log::warn!("[Theme] preview reload failed: {e}");
    }
    spawned.is_ok()
}

/// The new preview, once shown: right where the old one is in the window order, then the old one ends.
fn take_place(window: &slint::Window, (pid, old): (u32, isize)) {
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindow, SetWindowPos, GW_HWNDPREV, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE};
    unsafe {
        if let Some(me) = crate::shell::nofocus::hwnd(window) {
            let old = HWND(old as *mut _);
            // the window just above the old preview (or the top): we go directly below it = the old one's spot
            let above = GetWindow(old, GW_HWNDPREV).ok().filter(|h| *h != me);
            let _ = SetWindowPos(me, Some(above.unwrap_or(HWND_TOP)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        // only ever our own preview process: it records nothing and saves nothing
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 0);
            let _ = CloseHandle(h);
        }
    }
}

fn place(window: &slint::Window, at: Option<[i32; 4]>) {
    if let Some([x, y, w, h]) = at {
        window.set_position(slint::PhysicalPosition::new(x, y));
        window.set_size(slint::PhysicalSize::new(w.max(200) as u32, h.max(150) as u32));
    }
}

fn end_preview() -> slint::CloseRequestResponse {
    let _ = slint::quit_event_loop();
    slint::CloseRequestResponse::HideWindow
}

fn handle(window: &slint::Window) -> isize {
    crate::shell::nofocus::hwnd(window).map_or(0, |h| h.0 as isize)
}

fn geometry(window: &slint::Window) -> [i32; 4] {
    let (p, s) = (window.position(), window.size());
    [p.x, p.y, s.width as i32, s.height as i32]
}

pub fn run(Args { dir, at, replace }: Args) {
    if log::set_logger(&crate::STDERR_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let mut settings = startup::load_settings();
    settings.auto_start_recording = false;
    settings.start_minimized = false;
    settings.stats_shown = false;
    lag_core::i18n::set_language(settings.language.as_deref().unwrap_or("en"));
    if let Err(e) = ui_twin::load_builtin() {
        startup::fatal_message(&format!("Lag could not load its interface:\n\n{e}"));
        return;
    }
    let name = themes::read(&dir).map(|t| t.name).unwrap_or_else(|_| dir.display().to_string());
    let builtin = dir == ui_twin::builtin_dir();
    let loaded = if builtin { Ok(()) } else { themes::apply_dir(&dir) };

    // keep whatever is shown alive for the whole loop
    let mut keep: Vec<Box<dyn std::any::Any>> = Vec::new();
    // (geometry, native handle) of whatever is shown
    let window_geometry: Box<dyn Fn() -> ([i32; 4], isize)> = match loaded {
        Ok(()) => {
            let Ok(ui) = AppWindow::new() else { return };
            ui.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
            let ctx = app_ctx::AppCtx::new(settings);
            ctx.never_save();
            views::init_all(&ui, &ctx);
            // closing the preview ends it (Lag's own close hides into the tray, and a preview has no tray: the
            // process stayed alive invisible, owner 2026-11-22). Lag's title bar has its own close button
            // (MainVM.close-window, not a close request): it ends the preview too (QA 2026-11-22)
            ui.window().on_close_requested(end_preview);
            ui.global::<MainVM>().on_close_window(|| {
                let _ = end_preview();
            });
            // the status pill says what this window is: a preview, not a second Lag (QA 2026-11-22)
            ui.global::<MainVM>()
                .set_status_text(lag_core::i18n::format("ThemePreview_Status", &[&name]).unwrap_or_default().into());
            place(ui.window(), at);
            if ui.show().is_err() {
                return;
            }
            if let Some(r) = replace {
                // after its first frames, so the old preview gives way to a drawn window, not an empty one
                let weak = ui.as_weak();
                slint::Timer::single_shot(Duration::from_millis(350), move || {
                    if let Some(x) = weak.upgrade() {
                        take_place(x.window(), r);
                    }
                });
            }
            let weak = ui.as_weak();
            keep.push(Box::new((ui, ctx)));
            Box::new(move || weak.upgrade().map_or(([0, 0, 1360, 840], 0), |ui| (geometry(ui.window()), handle(ui.window()))))
        }
        Err(e) => {
            log::warn!("[Theme] preview of \"{name}\":\n{e}");
            let Ok(w) = ThemeErrorWindow::new() else { return };
            w.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
            w.set_theme_name(name.clone().into());
            // paths inside the theme as the author sees them (app.slint:37), not the full folder path
            let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
            w.set_message(e.replace(&prefix, "").into());
            place(w.window(), at);
            w.window().on_close_requested(end_preview);
            if w.show().is_err() {
                return;
            }
            if let Some(r) = replace {
                // after its first frames, so the old preview gives way to a drawn window, not an empty one
                let weak = w.as_weak();
                slint::Timer::single_shot(Duration::from_millis(350), move || {
                    if let Some(x) = weak.upgrade() {
                        take_place(x.window(), r);
                    }
                });
            }
            let weak = w.as_weak();
            keep.push(Box::new(w));
            Box::new(move || weak.upgrade().map_or(([0, 0, 860, 520], 0), |w| (geometry(w.window()), handle(w.window()))))
        }
    };

    // live reload: a changed file (a save, a new picture) -> a fresh preview that takes this one's place
    let seen = std::cell::Cell::new(fingerprint(&dir));
    let watcher = std::rc::Rc::new(slint::Timer::default());
    let watched = dir.clone();
    let stop = std::rc::Rc::downgrade(&watcher);
    watcher.start(slint::TimerMode::Repeated, Duration::from_millis(400), move || {
        let now = fingerprint(&watched);
        if now == seen.get() {
            return;
        }
        seen.set(now);
        // let the editor finish writing (several files, temp-file renames)
        let (dir, (at, hwnd)) = (watched.clone(), window_geometry());
        let stop = stop.clone();
        slint::Timer::single_shot(Duration::from_millis(250), move || {
            if relaunch(&dir, at, hwnd) {
                // the new preview ends this one; stop watching meanwhile, and end anyway if it never does
                if let Some(w) = stop.upgrade() {
                    w.stop();
                }
                slint::Timer::single_shot(Duration::from_secs(20), || {
                    let _ = slint::quit_event_loop();
                });
            }
        });
    });
    let _ = slint::run_event_loop_until_quit();
    drop(keep);
    // like --gpu-check: no thread-local teardown races at exit
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_changes_the_fingerprint() {
        let d = std::env::temp_dir().join(format!("lag-preview-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("art")).unwrap();
        std::fs::write(d.join("app.slint"), "a").unwrap();
        let a = fingerprint(&d);
        assert_eq!(a, fingerprint(&d), "stable while nothing changes");
        std::fs::write(d.join("app.slint"), "ab").unwrap();
        let b = fingerprint(&d);
        assert_ne!(a, b, "an edit");
        std::fs::write(d.join("art").join("bg.png"), "x").unwrap();
        assert_ne!(b, fingerprint(&d), "a new file in a subfolder");
        let _ = std::fs::remove_dir_all(&d);
    }
}
