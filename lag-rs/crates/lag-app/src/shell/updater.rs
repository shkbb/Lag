//! Port of `Services/AppUpdateService.cs` and SettingsViewModel's shared update flow.
//! All network/installer operations are explicit, worker-only calls. Unit tests use JSON fixtures.
//!
//! Since 2.2 (owner: "оновлення на початковий екран запуску... аби вона відкривалась вже оновленою"):
//! - a manual launch checks on the startup splash (`startup_check`); a newer Lag is downloaded there with its
//!   progress and installed before Lag opens (a click on the splash opens Lag now; the update waits);
//! - the install itself is shown by LagUpdate.exe (crates/lag-update, `hand_off`): the same splash with "Installing
//!   the update...", then it starts the updated Lag; no more silent vanishing;
//! - a start into the tray (Windows startup) and a check the splash gave up on only download (`background_fetch`):
//!   the next start installs what was downloaded (`cached_update`), never in the middle of a session;
//! - Settings > "Check for updates" (`manual_check`) downloads with its percentage and installs at once.
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
    /// `2.2.1` (the fourth part only when it is not 0), for the user.
    pub fn short(&self) -> String {
        let v = self.0;
        if v[3] == 0 {
            format!("{}.{}.{}", v[0], v[1], v[2])
        } else {
            self.to_string()
        }
    }
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
    /// the installer's size from the release (the download's progress when the server sends no length)
    pub size: Option<u64>,
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
                let size = asset.get("size").and_then(|v| v.as_u64()).filter(|n| *n > 0);
                return Some(UpdateInfo { version, setup_url: url.into(), size });
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
/// `progress(done, total)` after every chunk (total from the server's Content-Length, when it sends one).
fn download(
    url: &str,
    output: &mut impl std::io::Write,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), Box<dyn std::error::Error>> {
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
        let (mut total, mut length) = (0u32, 4u32);
        let total = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut total as *mut u32).cast()),
            &mut length,
            std::ptr::null_mut(),
        )
        .ok()
        .map(|_| total as u64)
        .filter(|n| *n > 0);
        let mut done = 0u64;
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
            done += read as u64;
            progress(done, total);
        }
    }
    Ok(())
}
pub fn get_latest() -> Option<UpdateInfo> {
    if cfg!(test) {
        return None;
    }
    let mut json = Vec::new();
    match download(&feed_url(), &mut json, &mut |_, _| {}) {
        Ok(()) => parse_release(&String::from_utf8_lossy(&json)),
        Err(e) => {
            log::info!("[AppUpdate] release lookup failed: {e}");
            None
        }
    }
}
/// `%TEMP%\Lag-Setup-<version>.exe`: where an installer is kept until it is installed.
fn installer_path(v: Version) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("Lag-Setup-{v}.exe"))
}

/// The version in an installer's file name (`lag-setup-2.2.1.0.exe`, any case).
fn installer_version(name: &str) -> Option<Version> {
    let n = name.to_ascii_lowercase();
    Version::parse(n.strip_prefix("lag-setup-")?.strip_suffix(".exe")?)
}

/// Downloads the installer of `update` (into `.part`, renamed once complete, so a broken download is never
/// installed); a complete one from before is used as it is. `progress(done, total)` on this thread.
pub fn fetch(update: &UpdateInfo, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<std::path::PathBuf, String> {
    if cfg!(test) {
        return Err("network disabled in tests".into());
    }
    let path = installer_path(update.version);
    let complete = std::fs::metadata(&path).ok().is_some_and(|m| m.len() > 0 && update.size.is_none_or(|s| s == m.len()));
    if complete {
        return Ok(path);
    }
    let part = path.with_extension("exe.part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let result = download(&update.setup_url, &mut file, &mut |done, total| progress(done, total.or(update.size)));
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&part);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    log::info!("[AppUpdate] downloaded {}", path.display());
    Ok(path)
}

/// A complete installer of a newer Lag, downloaded earlier (installed at this start, before Lag opens).
pub fn cached_update() -> Option<(Version, std::path::PathBuf)> {
    if cfg!(test) || detect_install() == InstallKind::DevOrPortable {
        return None;
    }
    let current = current_version()?;
    std::fs::read_dir(std::env::temp_dir())
        .ok()?
        .flatten()
        .filter_map(|e| Some((installer_version(&e.file_name().to_string_lossy())?, e.path())))
        .filter(|(v, p)| *v > current && std::fs::metadata(p).is_ok_and(|m| m.len() > 0))
        .max_by_key(|(v, _)| *v)
}

/// Installers that are not needed any more (this Lag or older, broken downloads); a newer one waits for the next
/// start. 2.0.0/2.0.1 left every one behind (~127 MB each).
fn remove_old_installers() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let current = current_version();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let stale = if name.starts_with("lag-setup-") && name.ends_with(".exe.part") {
            true
        } else if name.starts_with("lag-setup-") && name.ends_with(".exe") {
            installer_version(&name).is_none_or(|v| current.is_none_or(|c| v <= c))
        } else {
            false
        };
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            log::info!("[AppUpdate] removed old installer {name}");
        }
    }
}

/// Starts the install of `installer` and returns true: the caller ends Lag at once (the installer replaces it).
/// LagUpdate.exe (next to Lag.exe) shows it and starts the updated Lag; it runs from %TEMP%, as the install folder
/// is replaced. Without it, the installer runs silently and starts Lag itself (/AUTOSTART), as before 2.2.
pub fn hand_off(installer: &Path, mode: &str) -> bool {
    if cfg!(test) {
        return false;
    }
    use lag_core::i18n::tr;
    let exe = std::env::current_exe().ok();
    let helper = exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("LagUpdate.exe")).filter(|h| h.is_file());
    if let (Some(helper), Some(exe)) = (helper, exe.as_ref()) {
        let temp = std::env::temp_dir();
        let copy = [temp.join("LagUpdate.exe"), temp.join(format!("LagUpdate-{}.exe", std::process::id()))]
            .into_iter()
            .find(|c| std::fs::copy(&helper, c).is_ok());
        if let Some(copy) = copy {
            let version = installer
                .file_name()
                .and_then(|n| installer_version(&n.to_string_lossy()))
                .map(|v| v.short())
                .unwrap_or_default();
            let installing = lag_core::i18n::format("Splash_UpdateInstalling", &[&version]).unwrap_or_default();
            let mut cmd = std::process::Command::new(&copy);
            cmd.arg("--installer").arg(installer).arg("--app").arg(exe);
            cmd.args(["--wait-pid", &std::process::id().to_string(), "--mode", mode]);
            cmd.args(["--installing", &installing, "--starting", tr("Splash_Starting"), "--failed", tr("Update_InstallFailed")]);
            // end-to-end tests with a stand-in installer
            if let Ok(a) = std::env::var("LAG_UPDATE_INSTALLER_ARGS") {
                cmd.args(["--installer-args", &a]);
            }
            match cmd.spawn() {
                Ok(_) => {
                    log::info!("[AppUpdate] handed off to {} with {} — exiting.", copy.display(), installer.display());
                    return true;
                }
                Err(e) => log::warn!("[AppUpdate] LagUpdate.exe did not start: {e}"),
            }
        }
    }
    // the way before 2.2: the installer alone, silent, relaunching Lag
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::{
        Foundation::CloseHandle,
        UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
    };
    let file: Vec<u16> = installer.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: w!("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /AUTOSTART"),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            log::warn!("[AppUpdate] installer did not start: {e}");
            return false;
        }
        if !info.hProcess.is_invalid() {
            let _ = CloseHandle(info.hProcess);
        }
    }
    log::info!("[AppUpdate] handed off to installer {} (silent) — exiting.", installer.display());
    true
}

use std::sync::atomic::{AtomicBool, Ordering};
/// One check / download at a time: 2.0.0 started two at launch, and both installed.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// GitHub has answered this run already (the splash's check): the background one 3 s later does not ask again
/// (60 unauthenticated requests an hour per address).
static ANSWERED: AtomicBool = AtomicBool::new(false);

/// The latest release, remembering that GitHub answered.
fn lookup() -> Option<UpdateInfo> {
    let latest = get_latest();
    if latest.is_some() {
        ANSWERED.store(true, Ordering::SeqCst);
    }
    latest
}

/// What the startup check reports (on its worker thread).
pub enum Startup {
    /// up to date, offline, or the download failed: open Lag
    Nothing,
    /// a newer Lag: downloading it
    Found(Version),
    /// the download's progress, 0..1 (None: no size known)
    Progress(Option<f32>),
    /// downloaded: install it
    Ready(std::path::PathBuf),
}

/// The check on the startup splash: looks up the latest release and downloads a newer one. False when there is
/// nothing to check (a dev / portable copy, or a check already running).
pub fn startup_check(events: impl Fn(Startup) + Send + 'static) -> bool {
    if detect_install() == InstallKind::DevOrPortable || RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    std::thread::spawn(move || {
        remove_old_installers();
        let kind = detect_install();
        match (current_version(), lookup()) {
            (Some(current), Some(latest)) if update_wanted(kind, current, latest.version) => {
                events(Startup::Found(latest.version));
                let mut last = -1i32;
                let result = fetch(&latest, &mut |done, total| {
                    let p = total.map(|t| (done as f64 / t as f64).min(1.0) as f32);
                    let step = p.map_or(-2, |p| (p * 100.0) as i32);
                    if step != last {
                        last = step;
                        events(Startup::Progress(p));
                    }
                });
                match result {
                    Ok(path) => events(Startup::Ready(path)),
                    Err(e) => {
                        log::info!("[AppUpdate] download failed: {e}");
                        events(Startup::Nothing);
                    }
                }
            }
            _ => events(Startup::Nothing),
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
    true
}

/// Lag started into the tray, or the splash's check gave up: download a newer Lag for the next start, never
/// install it now (the user may be in a game).
pub fn background_fetch() {
    if ANSWERED.load(Ordering::SeqCst) {
        log::info!("[AppUpdate] already checked at this start");
        return;
    }
    if detect_install() == InstallKind::DevOrPortable || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        remove_old_installers();
        let kind = detect_install();
        if let (Some(current), Some(latest)) = (current_version(), lookup()) {
            if update_wanted(kind, current, latest.version) {
                match fetch(&latest, &mut |_, _| {}) {
                    Ok(_) => log::info!("[AppUpdate] {} is downloaded: it installs at the next start", latest.version.short()),
                    Err(e) => log::info!("[AppUpdate] background download failed: {e}"),
                }
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Settings > "Check for updates": `status` gets what to show (a translation key, or a ready text with the
/// percentage), on Slint's event loop; a newer Lag is downloaded and installed at once (Lag ends).
pub fn manual_check(mode: String, status: impl Fn(String) + Send + Clone + 'static) {
    let say = {
        let status = status.clone();
        move |text: String| {
            let status = status.clone();
            let _ = slint::invoke_from_event_loop(move || status(text));
        }
    };
    if detect_install() == InstallKind::DevOrPortable {
        say("Update_DevMode".into());
        return;
    }
    // a manual check during the automatic one says the update is on its way (a friend's 1.x showed "Update
    // failed: download" there, while the automatic update finished fine a minute later)
    if RUNNING.swap(true, Ordering::SeqCst) {
        say("Update_Downloading".into());
        return;
    }
    std::thread::spawn(move || {
        let kind = detect_install();
        let outcome = match (current_version(), get_latest()) {
            (Some(current), Some(latest)) if update_wanted(kind, current, latest.version) => {
                let mut last = -1i64;
                let result = fetch(&latest, &mut |done, total| {
                    let pct = total.map_or(-1, |t| (done * 100 / t.max(1)) as i64);
                    if pct != last {
                        last = pct;
                        let text = if pct >= 0 {
                            lag_core::i18n::format("Update_DownloadingPercent", &[&pct.to_string()]).unwrap_or_default()
                        } else {
                            "Update_Downloading".into()
                        };
                        say(text);
                    }
                });
                match result {
                    Ok(path) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if hand_off(&path, &mode) {
                                super::exit();
                            } else {
                                status("Update_Failed".into());
                            }
                        });
                        return;
                    }
                    Err(e) => {
                        log::info!("[AppUpdate] download failed: {e}");
                        "Update_Failed"
                    }
                }
            }
            (Some(_), Some(_)) => "Update_UpToDate",
            _ => "Update_Failed",
        };
        RUNNING.store(false, Ordering::SeqCst);
        say(outcome.into());
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
    fn installers_are_named_by_version() {
        let v = Version::parse("2.2.1").unwrap();
        assert_eq!(v.short(), "2.2.1");
        assert_eq!(Version::parse("2.2.1.3").unwrap().short(), "2.2.1.3");
        let name = installer_path(v).file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, "Lag-Setup-2.2.1.0.exe");
        assert_eq!(installer_version(&name), Some(v));
        assert_eq!(installer_version("lag-setup-2.2.1.0.EXE"), Some(v));
        assert_eq!(installer_version("Lag-Setup-2.2.1.0.exe.part"), None, "a broken download is never installed");
        assert_eq!(installer_version("Lag-Setup-x.exe"), None);
        let json = r#"{"tag_name":"v2.2.1","assets":[{"name":"Lag-win-Setup.exe","size":81234567,"browser_download_url":"https://example.invalid/setup"}]}"#;
        assert_eq!(parse_release(json).unwrap().size, Some(81234567));
    }
    #[test]
    fn location_comparison() {
        assert!(same_location(Path::new(r"C:\Program Files\Lag"), "c:\\PROGRAM FILES\\Lag\\"));
        assert!(!same_location(Path::new(r"C:\Lag"), r"C:\Lag2"));
    }
}
