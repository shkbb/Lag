//! Port of `Services/AppUpdateService.cs` and SettingsViewModel's shared update flow.
//! All network/installer operations are explicit, worker-only calls. Unit tests use JSON fixtures.
use std::{fmt, path::Path, time::Duration};
use windows::{
    core::{w, PCWSTR},
    Win32::{Networking::WinHttp::*, System::Registry::*},
};

const API_LATEST: &str = "https://api.github.com/repos/shkbb/Lag/releases/latest";
const SETUP_ASSET: &str = "Lag-win-Setup.exe";
/// The installer's uninstall key (installer/Lag.iss AppId=Lag).
const UNINSTALL_KEY: &str = "Lag_is1";

/// Test hooks for a real end-to-end update run against a local server and an isolated install
/// (scratchpad "LagTest" installers): `LAG_UPDATE_FEED` replaces the GitHub "latest release" URL and
/// `LAG_UPDATE_KEY` the uninstall key name. Unset = the real ones.
fn feed_url() -> String {
    std::env::var("LAG_UPDATE_FEED").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| API_LATEST.into())
}
fn uninstall_subkey() -> Vec<u16> {
    let key = std::env::var("LAG_UPDATE_KEY").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| UNINSTALL_KEY.into());
    format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{key}").encode_utf16().chain(Some(0)).collect()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    DevOrPortable,
    Installed,
    LegacyVelopack,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version([u32; 4]);
impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<_> = text.split('.').collect();
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let mut version = [0; 4];
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n = part.parse::<u32>().ok()?;
            if n > i32::MAX as u32 {
                return None;
            }
            version[i] = n;
        }
        Some(Self(version))
    }
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: Version,
    pub setup_url: String,
}
pub fn current_version() -> Option<Version> {
    parse_build_version(env!("CARGO_PKG_VERSION"))
}
fn parse_build_version(text: &str) -> Option<Version> {
    Version::parse(text.split(['+', '-']).next()?)
}
fn same_location(a: &Path, b: &str) -> bool {
    a.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.trim_end_matches(['\\', '/']))
}
pub fn detect_install() -> InstallKind {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else {
        return InstallKind::DevOrPortable;
    };
    if dir.file_name().is_some_and(|s| s.eq_ignore_ascii_case("current"))
        && dir.parent().is_some_and(|p| p.join("Update.exe").is_file())
    {
        return InstallKind::LegacyVelopack;
    }
    let subkey_text = uninstall_subkey();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        // Read only, both per-user and per-machine Inno installations.
        let mut len = 0;
        unsafe {
            let subkey = PCWSTR(subkey_text.as_ptr());
            if RegGetValueW(root, subkey, w!("InstallLocation"), RRF_RT_REG_SZ, None, None, Some(&mut len)).is_err() {
                continue;
            }
            let mut text = vec![0u16; (len as usize).div_ceil(2)];
            if RegGetValueW(
                root,
                subkey,
                w!("InstallLocation"),
                RRF_RT_REG_SZ,
                None,
                Some(text.as_mut_ptr().cast()),
                Some(&mut len),
            )
            .is_ok()
            {
                let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
                if same_location(&dir, &String::from_utf16_lossy(&text[..end])) {
                    return InstallKind::Installed;
                }
            }
        }
    }
    InstallKind::DevOrPortable
}
pub fn parse_release(json: &str) -> Option<UpdateInfo> {
    let doc: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = doc.get("tag_name")?.as_str()?.trim_start_matches(['v', 'V']);
    let version = Version::parse(tag)?;
    for asset in doc.get("assets")?.as_array()? {
        if asset.get("name").and_then(|v| v.as_str()).is_some_and(|name| name.eq_ignore_ascii_case(SETUP_ASSET)) {
            let url = asset.get("browser_download_url")?.as_str()?;
            if !url.is_empty() {
                return Some(UpdateInfo { version, setup_url: url.into() });
            }
        }
    }
    None
}
pub fn update_wanted(kind: InstallKind, current: Version, latest: Version) -> bool {
    match kind {
        InstallKind::DevOrPortable => false,
        InstallKind::Installed => latest > current,
        InstallKind::LegacyVelopack => latest >= current,
    }
}
struct Internet(*mut std::ffi::c_void);
impl Internet {
    fn new(handle: *mut std::ffi::c_void) -> windows::core::Result<Self> {
        if handle.is_null() {
            Err(windows::core::Error::from_thread())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}
/// Native Windows TLS/redirect support, without a C/C++ build dependency. Stream installers to disk.
fn download(url: &str, output: &mut impl std::io::Write) -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(test) {
        return Err("network disabled in tests".into());
    }
    let started = std::time::Instant::now();
    let url: Vec<u16> = url.encode_utf16().collect();
    unsafe {
        let mut parts = URL_COMPONENTS {
            dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
            dwHostNameLength: u32::MAX,
            dwUrlPathLength: u32::MAX,
            dwExtraInfoLength: u32::MAX,
            ..Default::default()
        };
        WinHttpCrackUrl(&url, 0, &mut parts)?;
        let mut host = std::slice::from_raw_parts(parts.lpszHostName.0, parts.dwHostNameLength as usize).to_vec();
        host.push(0);
        let mut path = Vec::new();
        if parts.dwUrlPathLength != 0 {
            path.extend_from_slice(std::slice::from_raw_parts(parts.lpszUrlPath.0, parts.dwUrlPathLength as usize));
        }
        if parts.dwExtraInfoLength != 0 {
            path.extend_from_slice(std::slice::from_raw_parts(parts.lpszExtraInfo.0, parts.dwExtraInfoLength as usize));
        }
        if path.is_empty() {
            path.push('/' as u16);
        }
        path.push(0);
        let session = Internet::new(WinHttpOpen(
            w!("Lag-Updater"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 300_000, 300_000, 300_000, 300_000)?;
        let connection = Internet::new(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), parts.nPort, 0))?;
        let flags = if parts.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS {
            WINHTTP_FLAG_SECURE
        } else {
            WINHTTP_OPEN_REQUEST_FLAGS(0)
        };
        let request = Internet::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let (mut status, mut length) = (0u32, 4u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut length,
            std::ptr::null_mut(),
        )?;
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}").into());
        }
        let mut buffer = [0u8; 65536];
        loop {
            // A stalled connection is caught by the 300 s WinHTTP receive timeout above; this cap only stops a
            // crawl. 2.0.0 capped the whole download at 300 s, so a 127 MB installer never finished below
            // ~3.5 Mbit/s and the update was retried (and lost) on every launch.
            if started.elapsed() > Duration::from_secs(3600) {
                return Err("download took over an hour".into());
            }
            let mut read = 0;
            WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read as usize])?;
        }
    }
    Ok(())
}
pub fn get_latest() -> Option<UpdateInfo> {
    if cfg!(test) {
        return None;
    }
    let mut json = Vec::new();
    match download(&feed_url(), &mut json) {
        Ok(()) => parse_release(&String::from_utf8_lossy(&json)),
        Err(e) => {
            log::info!("[AppUpdate] release lookup failed: {e}");
            None
        }
    }
}
/// Returns true after installer handoff. The UI caller must immediately call shell::exit().
pub fn download_and_install(update: &UpdateInfo) -> bool {
    if cfg!(test) {
        return false;
    }
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("Lag-Setup-{}.exe", update.version));
        let mut dst = std::fs::File::create(&path)?;
        download(&update.setup_url, &mut dst)?;
        drop(dst);
        // C# UseShellExecute=true also supports an installer requiring elevation.
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::{
            Foundation::CloseHandle,
            UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
        };
        let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: w!("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART"),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        unsafe {
            ShellExecuteExW(&mut info)?;
            if !info.hProcess.is_invalid() {
                let _ = CloseHandle(info.hProcess);
            }
        }
        log::info!("[AppUpdate] handed off to installer {} — exiting.", update.version);
        Ok(())
    })();
    if let Err(e) = result {
        log::info!("[AppUpdate] download/install failed: {e}");
        false
    } else {
        true
    }
}
/// Installers this updater downloaded earlier (`%TEMP%\Lag-Setup-<version>.exe`, ~127 MB each) are not needed
/// once the app runs again; 2.0.0/2.0.1 left every one behind. One still in use stays until the next launch.
fn remove_old_installers() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("lag-setup-") && name.ends_with(".exe") && std::fs::remove_file(entry.path()).is_ok() {
            log::info!("[AppUpdate] removed old installer {name}");
        }
    }
}
/// Invoke from Settings or a startup timer. No network until this API is called.
/// `finished` is delivered on Slint's event loop, including a localized status resource key.
pub fn check_for_updates(silent: bool, finished: impl FnOnce(Option<&'static str>) + Send + 'static) {
    use std::sync::atomic::{AtomicBool, Ordering};
    // One check at a time: 2.0.0 started two at launch (shell + settings page), and both downloaded and
    // launched the installer. A manual check during the automatic one says the update is on its way (a friend's
    // 1.x showed "Update failed: download" there, while the automatic update finished fine a minute later).
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        let _ = slint::invoke_from_event_loop(move || finished(if silent { None } else { Some("Update_Downloading") }));
        return;
    }
    std::thread::spawn(move || {
        if silent {
            remove_old_installers();
        }
        let kind = detect_install();
        let result = if kind == InstallKind::DevOrPortable {
            Some("Update_DevMode")
        } else if let (Some(current), Some(latest)) = (current_version(), get_latest()) {
            if update_wanted(kind, current, latest.version) {
                if download_and_install(&latest) {
                    // stays RUNNING: the app is exiting for the installer
                    let _ = slint::invoke_from_event_loop(super::exit);
                    return;
                } else {
                    Some("Update_Failed")
                }
            } else {
                Some("Update_UpToDate")
            }
        } else {
            Some("Update_UpToDate")
        };
        RUNNING.store(false, Ordering::SeqCst);
        let _ = slint::invoke_from_event_loop(move || finished(if silent { None } else { result }));
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions_and_migration() {
        assert_eq!(Version::parse("1.7"), Version::parse("1.7.0.0"));
        assert_eq!(parse_build_version("1.7.0+g23ea66d"), Version::parse("1.7"));
        assert_eq!(parse_build_version("1.7.0-beta"), Version::parse("1.7"));
        for text in ["1", "1..2", "1.2.3.4.5", "v1.2", "1.2-beta", "-1.2", "2147483648.0"] {
            assert!(Version::parse(text).is_none(), "{text}");
        }
        let a = Version::parse("1.7").unwrap();
        let b = Version::parse("1.8").unwrap();
        assert!(b > a);
        assert!(!update_wanted(InstallKind::Installed, a, a));
        assert!(update_wanted(InstallKind::LegacyVelopack, a, a));
        assert!(!update_wanted(InstallKind::LegacyVelopack, b, a));
        assert!(!update_wanted(InstallKind::DevOrPortable, a, b));
    }
    #[test]
    fn release_fixtures() {
        let json = r#"{"tag_name":"v1.8.0","assets":[{"name":"portable.zip","browser_download_url":"https://example.invalid/zip"},{"name":"LAG-WIN-SETUP.EXE","browser_download_url":"https://example.invalid/setup"}]}"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, Version::parse("1.8").unwrap());
        assert_eq!(release.setup_url, "https://example.invalid/setup");
        for json in [
            "{}",
            "broken",
            r#"{"tag_name":"v1.8-beta","assets":[]}"#,
            r#"{"tag_name":"1.8","assets":[{"name":"Lag-win-Setup.exe","browser_download_url":""}]}"#,
        ] {
            assert!(parse_release(json).is_none());
        }
    }
    #[test]
    fn location_comparison() {
        assert!(same_location(Path::new(r"C:\Program Files\Lag"), "c:\\PROGRAM FILES\\Lag\\"));
        assert!(!same_location(Path::new(r"C:\Lag"), r"C:\Lag2"));
    }
}
