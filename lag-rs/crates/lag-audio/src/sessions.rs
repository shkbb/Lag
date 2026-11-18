//! Mirrors Services/AudioSessionService.cs and process selection in WasapiAudioSource.cs.
use crate::{
    wasapi::{enumerator, wide, Apartment},
    Result,
};
use std::{collections::HashMap, path::Path};
use windows::{
    core::{Interface, BOOL, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        Media::Audio::*,
        Storage::FileSystem::*,
        System::{Com::CLSCTX_ALL, Diagnostics::ToolHelp::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
};
#[derive(Debug, Clone)]
pub struct AudioApp {
    pub exe_name: String,
    pub display_name: String,
    pub icon_png: Option<Vec<u8>>,
}
pub fn get_active_audio_apps() -> Vec<AudioApp> {
    let run = || -> Result<Vec<AudioApp>> {
        let _com = Apartment::mta()?;
        let d = unsafe { enumerator()?.GetDefaultAudioEndpoint(eRender, eMultimedia)? };
        let manager: IAudioSessionManager2 = unsafe { d.Activate(CLSCTX_ALL, None)? };
        let sessions = unsafe { manager.GetSessionEnumerator()? };
        let mut found = HashMap::new();
        for i in 0..unsafe { sessions.GetCount()? } {
            let get = || -> Result<Option<AudioApp>> {
                unsafe {
                    let session = sessions.GetSession(i)?;
                    if session.GetState()? != AudioSessionStateActive {
                        return Ok(None);
                    }
                    let pid = session.cast::<IAudioSessionControl2>()?.GetProcessId()?;
                    if pid == 0 || pid == GetCurrentProcessId() {
                        return Ok(None);
                    }
                    let path = process_image_path(pid);
                    let fallback = processes().into_iter().find(|p| p.0 == pid).map(|p| p.1);
                    let exe = path
                        .as_deref()
                        .and_then(|p| Path::new(p).file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .or(fallback);
                    let Some(exe) = exe else {
                        return Ok(None);
                    };
                    let name = Path::new(&exe).file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    let display_name = path.as_deref().and_then(file_description).unwrap_or(name);
                    let icon_png = path.as_deref().and_then(crate::sessions::icons::extract);
                    Ok(Some(AudioApp { exe_name: exe, display_name, icon_png }))
                }
            };
            if let Ok(Some(app)) = get() {
                found.entry(app.exe_name.to_lowercase()).or_insert(app);
            }
        }
        let mut apps: Vec<_> = found.into_values().collect();
        apps.sort_by_key(|a| a.display_name.to_lowercase());
        Ok(apps)
    };
    run().unwrap_or_else(|e| {
        log::debug!("[AudioSessionService] Session enumeration failed: {e}");
        Vec::new()
    })
}
pub fn process_image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut n = buffer.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok().map(|_| String::from_utf16_lossy(&buffer[..n as usize]))
    }
}
fn processes() -> Vec<(u32, String)> {
    unsafe {
        let Ok(h) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut out = Vec::new();
        if Process32FirstW(h, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|&v| v == 0).unwrap_or(e.szExeFile.len());
                out.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                if Process32NextW(h, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(h);
        out
    }
}
pub fn find_process(exe: &str) -> Option<u32> {
    let name = if exe.to_lowercase().ends_with(".exe") { exe.to_owned() } else { format!("{exe}.exe") };
    let candidates: Vec<u32> =
        processes().into_iter().filter(|(_, n)| n.eq_ignore_ascii_case(&name)).map(|p| p.0).collect();
    let mut state = (candidates, None);
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        let state = &mut *(param.0 as *mut (Vec<u32>, Option<u32>));
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if state.0.contains(&pid) && IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).is_err() {
            state.1 = Some(pid);
            return BOOL(0);
        }
        BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut state as *mut (Vec<u32>, Option<u32>)) as isize));
    }
    state.1.or_else(|| state.0.first().copied())
}
fn file_description(path: &str) -> Option<String> {
    unsafe {
        let path = wide(path);
        let n = GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None);
        if n == 0 {
            return None;
        }
        let mut data = vec![0u8; n as usize];
        GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, n, data.as_mut_ptr().cast()).ok()?;
        let mut p = std::ptr::null_mut();
        let mut len = 0;
        if !VerQueryValueW(data.as_ptr().cast(), windows::core::w!("\\VarFileInfo\\Translation"), &mut p, &mut len)
            .as_bool()
            || len < 4
        {
            return None;
        }
        let translations = std::slice::from_raw_parts(p.cast::<u16>(), len as usize / 2);
        for t in translations.as_chunks::<2>().0.iter() {
            let key = wide(&format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", t[0], t[1]));
            if VerQueryValueW(data.as_ptr().cast(), PCWSTR(key.as_ptr()), &mut p, &mut len).as_bool() && len > 1 {
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p.cast::<u16>(), len as usize - 1))
                    .trim()
                    .to_owned();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
}
mod icons {
    use super::*;
    use windows::Win32::{Graphics::Gdi::*, UI::Shell::*};
    pub fn extract(path: &str) -> Option<Vec<u8>> {
        unsafe {
            let path = wide(path);
            let mut info = SHFILEINFOW::default();
            if SHGetFileInfoW(
                PCWSTR(path.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Some(&mut info),
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            ) == 0
            {
                return None;
            }
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                let _ = DestroyIcon(info.hIcon);
                return None;
            }
            let bitmap = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: 32,
                    biHeight: -32,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = std::ptr::null_mut();
            let result = if let Ok(bmp) = CreateDIBSection(Some(dc), &bitmap, DIB_RGB_COLORS, &mut pixels, None, 0) {
                let old = SelectObject(dc, HGDIOBJ(bmp.0));
                std::ptr::write_bytes(pixels, 0, 4096);
                let ok = DrawIconEx(dc, 0, 0, info.hIcon, 32, 32, 0, None, DI_NORMAL).is_ok();
                let data = std::slice::from_raw_parts(pixels.cast::<u8>(), 4096).to_vec();
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                if ok {
                    png(&data)
                } else {
                    None
                }
            } else {
                None
            };
            let _ = DeleteDC(dc);
            let _ = DestroyIcon(info.hIcon);
            result
        }
    }
    unsafe fn png(bgra: &[u8]) -> Option<Vec<u8>> {
        use lag_ffi::ffmpeg as f;
        let codec = f::avcodec_find_encoder(f::AV_CODEC_ID_PNG);
        if codec.is_null() {
            return None;
        }
        let mut ctx = f::avcodec_alloc_context3(codec);
        let mut frame = f::av_frame_alloc();
        let mut packet = f::av_packet_alloc();
        let result = (|| {
            if ctx.is_null() || frame.is_null() || packet.is_null() {
                return None;
            }
            (*ctx).width = 32;
            (*ctx).height = 32;
            (*ctx).pix_fmt = f::AV_PIX_FMT_RGBA;
            (*ctx).time_base = f::AVRational { num: 1, den: 1 };
            if f::avcodec_open2(ctx, codec, std::ptr::null_mut()) < 0 {
                return None;
            }
            (*frame).width = 32;
            (*frame).height = 32;
            (*frame).format = f::AV_PIX_FMT_RGBA;
            if f::av_frame_get_buffer(frame, 0) < 0 {
                return None;
            }
            for y in 0..32 {
                for x in 0..32 {
                    let s = &bgra[(y * 32 + x) * 4..][..4];
                    let p = (*frame).data[0].add(y * (*frame).linesize[0] as usize + x * 4);
                    std::ptr::copy_nonoverlapping([s[2], s[1], s[0], s[3]].as_ptr(), p, 4);
                }
            }
            if f::avcodec_send_frame(ctx, frame) < 0 || f::avcodec_receive_packet(ctx, packet) < 0 {
                return None;
            }
            Some(std::slice::from_raw_parts((*packet).data, (*packet).size as usize).to_vec())
        })();
        f::av_packet_free(&mut packet);
        f::av_frame_free(&mut frame);
        f::avcodec_free_context(&mut ctx);
        result
    }
}
