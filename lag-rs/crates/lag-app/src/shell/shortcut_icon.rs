//! The icon of Lag's shortcuts (Start menu, desktop, pinned to the taskbar) follows the theme.
//!
//! The taskbar button of a running app takes its picture from the Start-menu shortcut, not from the window:
//! WM_SETICON (tray.rs) changed the title bar and Alt+Tab, but the taskbar stayed dark (owner, 2026-11-20,
//! confirmed by pointing his Start-menu shortcut at the GTA icon by hand). Lag.exe carries every theme's
//! icon (build.rs), so a shortcut only needs a different icon index - no files to write or clean up, and it
//! works wherever Lag is installed. Only shortcuts that point at this very Lag.exe are touched; the
//! installer recreates the Start-menu one on every update, and the next launch themes it again.
use super::tray::IconStyle;
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Mutex, OnceLock},
    time::Duration,
};
use windows::{
    core::{Interface, HSTRING, PCWSTR},
    Win32::{
        System::Com::*,
        UI::Shell::*,
    },
};

/// Shortcut icon index per theme: 0 = the exe's first icon (dark); a negative index is a resource ID
/// (build.rs: 2 = light, 3 = GTA VI).
fn icon_index(style: IconStyle) -> i32 {
    match style {
        IconStyle::Dark => 0,
        IconStyle::Light => -2,
        IconStyle::Gta => -3,
    }
}

/// A theme change waits this long for the next one: "system" can resolve a moment after start, and a user
/// clicking through themes should cause one rewrite, not five.
const SETTLE: Duration = Duration::from_millis(1500);

static WORKER: OnceLock<mpsc::Sender<IconStyle>> = OnceLock::new();
static LAST_SENT: Mutex<Option<IconStyle>> = Mutex::new(None);

/// Themes the shortcuts in the background (shell COM calls can take a while; never on the UI thread).
/// Called every second by the tray's theme watcher: only a change reaches the worker (a steady stream would
/// keep restarting its settle wait).
pub(crate) fn follow(style: IconStyle) {
    if cfg!(test) {
        return;
    }
    if LAST_SENT.lock().map(|mut last| last.replace(style) == Some(style)).unwrap_or(true) {
        return;
    }
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("shortcut-icons".into()).spawn(move || worker(rx));
        tx
    });
    let _ = tx.send(style);
}

fn worker(rx: mpsc::Receiver<IconStyle>) {
    let Ok(exe) = std::env::current_exe() else { return };
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
        return;
    }
    let mut applied = None;
    while let Ok(mut style) = rx.recv() {
        while let Ok(next) = rx.recv_timeout(SETTLE) {
            style = next;
        }
        if applied == Some(style) {
            continue;
        }
        let changed = shortcut_folders()
            .iter()
            .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
            .map(|entry| entry.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")))
            .filter(|lnk| match retheme(lnk, &exe, icon_index(style)) {
                Ok(changed) => changed,
                Err(e) => {
                    // all-users shortcuts are read-only for a per-user process: expected, not an error
                    log::debug!("Shortcut icon: {} skipped: {e}", lnk.display());
                    false
                }
            })
            .count();
        if changed > 0 {
            log::info!("Shortcut icon: {changed} shortcut(s) now use the {style:?} icon");
            // the taskbar and Start keep icons in the shell's cache until told the associations changed
            unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
        }
        applied = Some(style);
    }
}

/// Start menu, desktop and taskbar pins, the user's and the all-users ones.
fn shortcut_folders() -> Vec<PathBuf> {
    let known = |id: &windows::core::GUID| -> Option<PathBuf> {
        unsafe {
            let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let path = p.to_string().ok().map(PathBuf::from);
            CoTaskMemFree(Some(p.0 as *const _));
            path
        }
    };
    [
        known(&FOLDERID_Programs),
        known(&FOLDERID_Desktop),
        known(&FOLDERID_UserPinned).map(|p| p.join("TaskBar")),
        known(&FOLDERID_CommonPrograms),
        known(&FOLDERID_PublicDesktop),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Points `lnk`'s icon at `exe`,`index` if `lnk` launches `exe`. Ok(true) when it was rewritten.
fn retheme(lnk: &Path, exe: &Path, index: i32) -> windows::core::Result<bool> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        let lnk_w = HSTRING::from(lnk.as_os_str());
        file.Load(&lnk_w, STGM_READWRITE)?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        if !same_file(&wide_to_path(&target), exe) {
            return Ok(false);
        }
        let mut icon = [0u16; 1024];
        let mut current = 0;
        link.GetIconLocation(&mut icon, &mut current)?;
        let icon = wide_to_path(&icon);
        let unchanged = if icon.as_os_str().is_empty() { index == 0 } else { current == index && same_file(&icon, exe) };
        if unchanged {
            return Ok(false);
        }
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), index)?;
        file.Save(PCWSTR::null(), true)?;
        SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(lnk_w.as_ptr() as *const _), None);
        Ok(true)
    }
}

fn wide_to_path(buf: &[u16]) -> PathBuf {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    PathBuf::from(String::from_utf16_lossy(&buf[..n]))
}

/// The same file, whatever the case, slashes or 8.3 names.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_has_its_own_icon_in_the_exe() {
        let all = [IconStyle::Dark, IconStyle::Light, IconStyle::Gta].map(icon_index);
        assert_eq!(all, [0, -2, -3], "must match the resource IDs build.rs embeds");
    }

    /// The real shell round trip on a throwaway shortcut in the temp folder (never the user's own ones).
    #[test]
    fn rethemes_only_shortcuts_to_this_exe() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let dir = std::env::temp_dir().join(format!("lag-shortcut-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        let make = |name: &str, target: &Path| -> PathBuf {
            let path = dir.join(name);
            unsafe {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
                link.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(path.as_os_str()), true).unwrap();
            }
            path
        };
        let ours = make("Lag.lnk", &exe);
        let other = make("Other.lnk", Path::new(r"C:\Windows\notepad.exe"));

        assert!(!retheme(&ours, &exe, 0).unwrap(), "a fresh shortcut already shows the dark (first) icon");
        assert!(retheme(&ours, &exe, -3).unwrap(), "GTA rewrites it");
        assert!(!retheme(&ours, &exe, -3).unwrap(), "and only once");
        assert!(retheme(&ours, &exe, 0).unwrap(), "back to dark");
        assert!(!retheme(&other, &exe, -3).unwrap(), "someone else's shortcut is never touched");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
