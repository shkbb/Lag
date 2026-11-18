//! "Import": the system Open dialog for any video file.

use super::tr;

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
