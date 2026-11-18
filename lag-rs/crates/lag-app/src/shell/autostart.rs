//! Port of `ViewModels/SettingsViewModel.cs` ApplyStartWithWindows / ApplyDisableGameMode.
use crate::app_ctx::AppCtx;
use std::{cell::Cell, rc::Rc};
use windows::{
    core::{w, PCWSTR},
    Win32::{Foundation::ERROR_FILE_NOT_FOUND, System::Registry::*},
};

/// Listen for actual setting transitions; initialization and unrelated edits never write registry.
pub fn init(ctx: &Rc<AppCtx>) {
    let s = ctx.settings();
    let previous = Cell::new((s.start_with_windows, s.disable_game_mode));
    ctx.on_settings_changed(move |s| {
        let next = (s.start_with_windows, s.disable_game_mode);
        let old = previous.replace(next);
        if old.0 != next.0 {
            if let Err(e) = apply_start_with_windows(next.0) {
                log::warn!("Failed to update 'Start with Windows' registry entry: {e}");
            }
        }
        if old.1 != next.1 {
            if let Err(e) = apply_disable_game_mode(next.1) {
                log::warn!("Failed to toggle Windows Game Mode: {e}");
            }
        }
    });
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
fn apply_start_with_windows(enable: bool) -> windows::core::Result<()> {
    // Never exercised by headless tests; the test configuration cannot write these values.
    if cfg!(test) {
        return Ok(());
    }
    unsafe {
        let mut key = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            KEY_SET_VALUE,
            &mut key,
        );
        if result == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        result.ok()?;
        let key = Key(key);
        if enable {
            if let Ok(exe) = std::env::current_exe() {
                let text = format!("\"{}\"", exe.display());
                let data: Vec<u8> = text.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect();
                RegSetValueExW(key.0, w!("Lag"), None, REG_SZ, Some(&data)).ok()?;
            }
        } else {
            let status = RegDeleteValueW(key.0, w!("Lag"));
            if status != ERROR_FILE_NOT_FOUND {
                status.ok()?;
            }
        }
    }
    Ok(())
}
fn apply_disable_game_mode(disable: bool) -> windows::core::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("SOFTWARE\\Microsoft\\GameBar"),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let key = Key(key);
        let data = u32::from(!disable).to_le_bytes();
        for name in [w!("AllowAutoGameMode"), w!("AutoGameModeEnabled")] {
            RegSetValueExW(key.0, name, None, REG_DWORD, Some(&data)).ok()?;
        }
    }
    Ok(())
}
