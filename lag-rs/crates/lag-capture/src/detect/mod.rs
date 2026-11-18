//! Port of `Services/VfrCapture/GameDetector.cs`: positive game-window classification.
use crate::overlay::gpu_usage::GpuUsageProbe;
use std::collections::HashSet;
use windows::core::{w, BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameCandidate {
    pub hwnd: isize,
    pub pid: u32,
    pub exe: String,
    pub game_name: String,
    pub score: i32,
    pub fullscreen: bool,
}
const KNOWN: &[(&str, &str)] = &[
    ("cs2", "Counter-Strike 2"),
    ("csgo", "CS:GO"),
    ("dota2", "Dota 2"),
    ("deadlock", "Deadlock"),
    ("valorant-win64-shipping", "VALORANT"),
    ("valorant", "VALORANT"),
    ("r5apex", "Apex Legends"),
    ("r5apexdx12", "Apex Legends"),
    ("fortniteclient-win64-shipping", "Fortnite"),
    ("gta5", "GTA V"),
    ("gtav", "GTA V"),
    ("gta-v", "GTA V"),
    ("rocketleague", "Rocket League"),
    ("league of legends", "League of Legends"),
    ("leagueoflegends", "League of Legends"),
    ("overwatch", "Overwatch 2"),
    ("tslgame", "PUBG: BATTLEGROUNDS"),
    ("rainbowsix", "Rainbow Six Siege"),
    ("rainbowsix_vulkan", "Rainbow Six Siege"),
    ("destiny2", "Destiny 2"),
    ("bf2042", "Battlefield 2042"),
    ("bf1", "Battlefield 1"),
    ("bfv", "Battlefield V"),
    ("eldenring", "Elden Ring"),
    ("marvel-win64-shipping", "Marvel Rivals"),
    ("discovery", "THE FINALS"),
    ("helldivers2", "Helldivers 2"),
    ("palworld-win64-shipping", "Palworld"),
    ("palworld", "Palworld"),
    ("rust", "Rust"),
    ("dayz", "DayZ"),
    ("warthunder", "War Thunder"),
    ("aces", "War Thunder"),
    ("modernwarfare", "Call of Duty"),
    ("cod", "Call of Duty"),
    ("minecraft.windows", "Minecraft"),
    ("hl2", "Half-Life 2"),
    ("portal2", "Portal 2"),
];
const ENGINES: &[&str] = &["UnrealWindow", "SDL_app", "riotWindowClass", "CryENGINE", "Valve001", "Sledgehammer Oasis"];
const SHELL: &[&str] = &[
    "explorer.exe",
    "dwm.exe",
    "applicationframehost.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "lockapp.exe",
    "sihost.exe",
    "ctfmon.exe",
    "lag.exe",
];
const NON_GAME: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "librewolf.exe",
    "brave.exe",
    "opera.exe",
    "opera_gx.exe",
    "vivaldi.exe",
    "browser.exe",
    "arc.exe",
    "discord.exe",
    "discordcanary.exe",
    "discordptb.exe",
    "slack.exe",
    "ms-teams.exe",
    "teams.exe",
    "spotify.exe",
    "code.exe",
    "claude.exe",
    "notion.exe",
    "whatsapp.exe",
    "telegram.exe",
    "obs64.exe",
    "obs32.exe",
    "obs.exe",
    "streamlabs obs.exe",
    "xsplit.core.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "mpc-hc.exe",
    "mpc-be64.exe",
    "potplayermini64.exe",
    "potplayer64.exe",
    "mpv.exe",
    "wmplayer.exe",
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "battle.net.exe",
    "galaxyclient.exe",
    "eadesktop.exe",
    "ealauncher.exe",
    "riotclientux.exe",
    "riotclientservices.exe",
];
fn classify(
    exe: &str,
    class: &str,
    title: &str,
    fullscreen: bool,
    focused: bool,
    gpu: f64,
    steam: bool,
) -> Option<(String, i32)> {
    if SHELL.iter().chain(NON_GAME).any(|s| s.eq_ignore_ascii_case(exe)) {
        return None;
    }
    let proc = if exe.to_ascii_lowercase().ends_with(".exe") { &exe[..exe.len() - 4] } else { exe };
    let known = KNOWN.iter().find(|(key, _)| key.eq_ignore_ascii_case(proc)).map(|(_, name)| *name);
    let engine = ENGINES.iter().any(|s| s.eq_ignore_ascii_case(class));
    let gpu3d = gpu >= 30.0;
    if known.is_none() && !(fullscreen && (gpu3d || steam || engine)) {
        return None;
    }
    let score = i32::from(known.is_some()) * 46000
        + i32::from(fullscreen) * 30000
        + i32::from(focused) * 18000
        + i32::from(gpu3d) * 46000
        + i32::from(steam) * 30000;
    let name = known.unwrap_or_else(|| if title.trim().is_empty() { proc } else { title.trim() });
    Some((name.to_owned(), score))
}
#[derive(Default)]
pub struct GameDetector {
    gpu: GpuUsageProbe,
    steam_game_running: bool,
}
impl GameDetector {
    pub fn new() -> Self {
        Self::default()
    }
    fn scan(&mut self, monitor_device_id: Option<&str>) -> Vec<GameCandidate> {
        self.gpu.sample();
        self.steam_game_running = steam_game_running();
        struct Scan<'a> {
            detector: &'a GameDetector,
            monitor: Option<&'a str>,
            foreground: isize,
            result: Vec<GameCandidate>,
        }
        unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
            // EnumWindows is synchronous; the stack context outlives every callback.
            let scan = unsafe { &mut *(param.0 as *mut Scan<'_>) };
            if let Some(c) = scan.detector.evaluate(hwnd.0 as isize, scan.monitor, scan.foreground) {
                scan.result.push(c);
            }
            BOOL(1)
        }
        let mut scan = Scan {
            detector: self,
            monitor: monitor_device_id,
            foreground: unsafe { GetForegroundWindow().0 as isize },
            result: Vec::new(),
        };
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut scan as *mut _ as isize));
        }
        scan.result
    }
    pub fn find_best_game(&mut self, monitor_device_id: Option<&str>) -> Option<GameCandidate> {
        self.scan(monitor_device_id).into_iter().reduce(|best, c| if c.score > best.score { c } else { best })
    }
    /// Names normalized for case-insensitive audio-picker membership.
    pub fn running_game_exes(&mut self, monitor_device_id: Option<&str>) -> HashSet<String> {
        self.scan(monitor_device_id).into_iter().map(|c| c.exe.to_lowercase()).collect()
    }
    pub fn evaluate(&self, hwnd: isize, monitor_device_id: Option<&str>, foreground: isize) -> Option<GameCandidate> {
        let window = HWND(hwnd as *mut _);
        unsafe {
            if hwnd == 0 || !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if pid == 0 || pid == std::process::id() {
                return None;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut path = [0u16; 32768];
            let mut len = path.len() as u32;
            let queried = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            queried.ok()?;
            let path = String::from_utf16_lossy(&path[..len as usize]);
            let exe = path.rsplit(['\\', '/']).next()?.to_owned();
            let mut class = [0u16; 256];
            let n = GetClassNameW(window, &mut class).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            let mut title = [0u16; 256];
            let n = GetWindowTextW(window, &mut title).max(0) as usize;
            let title = String::from_utf16_lossy(&title[..n]);
            let fullscreen = is_fullscreen(window, monitor_device_id);
            let (game_name, score) = classify(
                &exe,
                &class,
                &title,
                fullscreen,
                hwnd == foreground,
                self.gpu.get_3d(pid),
                self.steam_game_running,
            )?;
            Some(GameCandidate { hwnd, pid, exe, game_name, score, fullscreen })
        }
    }
}
fn covers(rect: RECT, monitor: RECT, flags: u32, device: &str, selected: Option<&str>) -> bool {
    let on_recorded = match selected.filter(|s| !s.is_empty()) {
        Some(s) => s.eq_ignore_ascii_case(device),
        None => flags & 1 != 0,
    };
    on_recorded
        && rect.left <= monitor.left
        && rect.top <= monitor.top
        && rect.right >= monitor.right
        && rect.bottom >= monitor.bottom
}
fn is_fullscreen(hwnd: HWND, selected: Option<&str>) -> bool {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info.monitorInfo).as_bool() {
            return false;
        }
        let n = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        covers(
            rect,
            info.monitorInfo.rcMonitor,
            info.monitorInfo.dwFlags,
            &String::from_utf16_lossy(&info.szDevice[..n]),
            selected,
        )
    }
}
fn steam_game_running() -> bool {
    unsafe {
        let (mut value, mut size) = (0u32, 4u32);
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Valve\Steam"),
            w!("RunningAppID"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .is_ok()
            && value != 0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qualification_scores_and_names() {
        assert_eq!(
            classify("CS2.EXE", "", "ignored", false, true, 0.0, false),
            Some(("Counter-Strike 2".into(), 64000))
        );
        assert!(classify("unknown.exe", "SDL_app", "game", false, true, 100.0, false).is_none());
        assert!(classify("unknown.exe", "", "game", true, true, 29.99, false).is_none());
        assert_eq!(classify("unknown.exe", "", "  Title  ", true, true, 30.0, true), Some(("Title".into(), 124000)));
        assert_eq!(
            classify("unknown.exe", "UnrealWindow", " \t", true, false, 0.0, false),
            Some(("unknown".into(), 30000))
        );
        assert_eq!(classify("cs2.exe", "", "", true, true, 99.0, true).map(|c| c.1), Some(170000));
        for exe in SHELL.iter().chain(NON_GAME) {
            assert!(classify(&exe.to_uppercase(), "SDL_app", "", true, true, 100.0, true).is_none());
        }
        for (exe, name) in KNOWN {
            assert_eq!(
                classify(&format!("{exe}.exe"), "", "", false, false, 0.0, false),
                Some((name.to_string(), 46000))
            );
        }
        for class in ENGINES {
            assert!(classify("unknown.exe", class, "", true, false, 0.0, false).is_some());
        }
    }
    #[test]
    fn monitor_filter_and_exact_edges() {
        let monitor = RECT { left: -1920, top: 0, right: 0, bottom: 1080 };
        assert!(covers(monitor, monitor, 1, "DISPLAY1", None));
        assert!(covers(monitor, monitor, 0, "DISPLAY2", Some("display2")));
        assert!(!covers(monitor, monitor, 0, "DISPLAY2", None));
        assert!(!covers(monitor, monitor, 1, "DISPLAY1", Some("DISPLAY2")));
        assert!(!covers(RECT { left: -1919, ..monitor }, monitor, 1, "DISPLAY1", None));
    }
    #[test]
    fn enumerates_real_windows_headlessly() {
        let _ = GameDetector::new().find_best_game(None);
    }
}
