//! Folder picker, Explorer, the diagnostics zip.

use super::*;
use std::path::Path;

pub(super) fn browse_folder(current: &str) -> Option<String> {
    use windows::{
        core::PCWSTR,
        Win32::{System::Com::*, UI::Shell::*},
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().ok()?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _com = Apartment;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR).ok()?;
        let title: Vec<u16> = tr("Browse_Title").encode_utf16().chain(Some(0)).collect();
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        let folder: Vec<u16> = current.encode_utf16().chain(Some(0)).collect();
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(folder.as_ptr()), None) {
            let _ = dialog.SetFolder(&item);
        }
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let result = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast()));
        result
    }
}
/// System Open (`save` = None) or Save As (`save` = Some(default file name)) dialog for one `.lagtheme` file.
/// None = cancelled.
pub(super) fn pick_theme_file(save: Option<&str>) -> Option<String> {
    pick_file(save, &tr("Theme_FileFilter"), "*.lagtheme", "lagtheme")
}

/// System Open / Save As dialog for one file of the kind `pattern` (`*.png;*.jpg`), named `filter` in the list;
/// `ext` is added to a saved name without one. None = cancelled. Blocks: call it off the UI thread.
pub(crate) fn pick_file(save: Option<&str>, filter: &str, pattern: &str, ext: &str) -> Option<String> {
    use windows::{
        core::{Interface, PCWSTR},
        Win32::{System::Com::*, UI::Shell::Common::COMDLG_FILTERSPEC, UI::Shell::*},
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
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (name, pattern) = (wide(filter), wide(pattern));
        let spec = [COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(pattern.as_ptr()) }];
        let dialog: IFileDialog = match save {
            Some(default) => {
                let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
                let file = wide(default);
                let _ = d.SetFileName(PCWSTR(file.as_ptr()));
                let ext = wide(ext);
                let _ = d.SetDefaultExtension(PCWSTR(ext.as_ptr()));
                d.cast().ok()?
            }
            None => {
                let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
                let _ = d.SetOptions(FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR);
                d.cast().ok()?
            }
        };
        let _ = dialog.SetFileTypes(&spec);
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let result = path.to_string().ok();
        CoTaskMemFree(Some(path.0.cast()));
        result
    }
}

pub(super) fn open_in_explorer(path: &Path, select: bool) {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("explorer.exe");
    if select {
        command.arg(format!("/select,{}", path.display()));
    } else {
        command.arg(path);
    }
    let _ = command.creation_flags(0x08000000).spawn();
}
/// Minimal ZIP STORE writer: avoids staging/copying logs and streams each file only once.
/// Records UTF-8 names and CRC32; no new compression dependency is needed by this package.
pub(super) fn diagnostics_zip(directory: &Path, destination: &Path) -> std::io::Result<()> {
    use std::io::{Read, Seek, Write};
    fn u16le(w: &mut impl Write, n: u16) -> std::io::Result<()> {
        w.write_all(&n.to_le_bytes())
    }
    fn u32le(w: &mut impl Write, n: u32) -> std::io::Result<()> {
        w.write_all(&n.to_le_bytes())
    }
    let mut zip = std::fs::File::create(destination)?;
    let mut central = Vec::new();
    let mut count = 0u16;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned().into_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| std::io::Error::other("diagnostic filename too long"))?;
        let offset = u32::try_from(zip.stream_position()?)
            .map_err(|_| std::io::Error::other("diagnostics exceed ZIP32 size"))?;
        let mut input = std::fs::File::open(entry.path())?;
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        let size = u32::try_from(bytes.len()).map_err(|_| std::io::Error::other("log exceeds ZIP32 size"))?;
        let mut crc = !0u32;
        for b in &bytes {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
            }
        }
        crc = !crc;
        u32le(&mut zip, 0x04034b50)?;
        for v in [20, 0x800, 0, 0, 0x21] {
            u16le(&mut zip, v)?;
        }
        u32le(&mut zip, crc)?;
        u32le(&mut zip, size)?;
        u32le(&mut zip, size)?;
        u16le(&mut zip, name_len)?;
        u16le(&mut zip, 0)?;
        zip.write_all(&name)?;
        zip.write_all(&bytes)?;
        u32le(&mut central, 0x02014b50)?;
        for v in [20, 20, 0x800, 0, 0, 0x21] {
            u16le(&mut central, v)?;
        }
        u32le(&mut central, crc)?;
        u32le(&mut central, size)?;
        u32le(&mut central, size)?;
        for v in [name_len, 0, 0, 0, 0] {
            u16le(&mut central, v)?;
        }
        u32le(&mut central, 0)?;
        u32le(&mut central, offset)?;
        central.write_all(&name)?;
        count = count.checked_add(1).ok_or_else(|| std::io::Error::other("too many diagnostic files"))?;
    }
    let offset =
        u32::try_from(zip.stream_position()?).map_err(|_| std::io::Error::other("diagnostics exceed ZIP32 size"))?;
    zip.write_all(&central)?;
    u32le(&mut zip, 0x06054b50)?;
    u16le(&mut zip, 0)?;
    u16le(&mut zip, 0)?;
    u16le(&mut zip, count)?;
    u16le(&mut zip, count)?;
    u32le(&mut zip, central.len() as u32)?;
    u32le(&mut zip, offset)?;
    u16le(&mut zip, 0)?;
    zip.flush()
}
pub(super) fn export_diagnostics() -> std::io::Result<()> {
    let logs =
        lag_core::filelog::log_directory().ok_or_else(|| std::io::Error::other("Documents directory unavailable"))?;
    let parent = logs.parent().ok_or_else(|| std::io::Error::other("Invalid logs path"))?;
    std::fs::create_dir_all(&logs)?;
    let target = parent.join(format!("Lag-diagnostics-{}.zip", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")));
    diagnostics_zip(&logs, &target)?;
    open_in_explorer(&target, true);
    Ok(())
}
