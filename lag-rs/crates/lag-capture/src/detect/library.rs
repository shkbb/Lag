//! Where games come from, without keeping a list of our own (owner, 2026-11-20: "find universal ways, a list
//! of banned apps never ends"):
//! * the games Windows itself recognised: Game Bar / Game Mode keep them in HKCU\System\GameConfigStore
//!   (Microsoft's known-games list matched at launch, plus what the user marked "remember this is a game");
//! * the folders game launchers install into: Steam libraries (any drive), Epic, GOG, Ubisoft, EA, Riot, Xbox.
//!
//! Read from the registry / disk at most every 30 s.
use std::collections::HashSet;
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::System::Registry::*;

const REFRESH: Duration = Duration::from_secs(30);

/// Folder names the launchers install games under, wherever the library is.
const LAUNCHER_MARKERS: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\gog galaxy\\games\\",
    "\\gog games\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
    "\\riot games\\",
    "\\xboxgames\\",
];
/// Installers and redistributables that live in those folders but are not the game (e.g. the DirectX /
/// VC++ setup Steam runs before a game's first start).
const NOT_GAMES: &[&str] = &["\\steamworks shared\\", "\\_commonredist\\", "\\redist\\", "\\redistributables\\"];

#[derive(Default)]
pub(super) struct GameLibrary {
    /// Lower-case full exe paths Windows knows as games.
    windows_games: HashSet<String>,
    /// Lower-case install folders read from launcher manifests (custom library locations).
    install_roots: Vec<String>,
    loaded_at: Option<Instant>,
}

impl GameLibrary {
    pub(super) fn refresh_if_stale(&mut self) {
        if self.loaded_at.is_some_and(|t| t.elapsed() < REFRESH) {
            return;
        }
        self.windows_games = read_windows_game_list();
        self.install_roots = read_launcher_roots();
        self.loaded_at = Some(Instant::now());
    }

    /// Windows lists this exe as a game.
    pub(super) fn is_windows_game(&self, path: &str) -> bool {
        self.windows_games.contains(&normalize(path))
    }

    /// The exe was installed by a game launcher.
    pub(super) fn is_launcher_install(&self, path: &str) -> bool {
        let p = normalize(path);
        if NOT_GAMES.iter().any(|m| p.contains(m)) {
            return false;
        }
        has_launcher_marker(&p) || self.install_roots.iter().any(|root| p.starts_with(root.as_str()))
    }
}

/// Lower case, back slashes: the form every comparison here uses.
fn normalize(path: &str) -> String {
    path.trim().to_lowercase().replace('/', "\\")
}

fn has_launcher_marker(normalized: &str) -> bool {
    LAUNCHER_MARKERS.iter().any(|m| normalized.contains(m))
}

/// A folder as a path prefix ("d:\games\fortnite\").
fn as_root(folder: &str) -> Option<String> {
    let f = normalize(folder);
    let f = f.trim_end_matches('\\');
    // a drive root or an empty value would make every program a game
    (f.len() > 3).then(|| format!("{f}\\"))
}

/// `"InstallLocation": "D:\\Epic Games\\Fortnite"` from an Epic manifest (*.item, JSON), unescaped.
fn epic_install_location(item: &str) -> Option<String> {
    let key = item.find("\"InstallLocation\"")?;
    let rest = &item[key + "\"InstallLocation\"".len()..];
    let open = rest.find('"')?;
    if !rest[..open].trim().trim_start_matches(':').trim().is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest[open + 1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
    None
}

fn read_windows_game_list() -> HashSet<String> {
    reg_subkey_values(HKEY_CURRENT_USER, r"System\GameConfigStore\Children", "MatchedExeFullPath")
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .map(|p| normalize(&p))
        .collect()
}

fn read_launcher_roots() -> Vec<String> {
    let mut roots = Vec::new();
    // Epic: one manifest per installed game
    if let Some(data) = std::env::var_os("ProgramData") {
        let dir = std::path::Path::new(&data).join(r"Epic\EpicGamesLauncher\Data\Manifests");
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("item")) {
                    if let Some(loc) = std::fs::read_to_string(e.path()).ok().as_deref().and_then(epic_install_location)
                    {
                        roots.extend(as_root(&loc));
                    }
                }
            }
        }
    }
    // GOG and Ubisoft keep install folders in the registry
    for (key, value) in [
        (r"SOFTWARE\WOW6432Node\GOG.com\Games", "path"),
        (r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs", "InstallDir"),
    ] {
        roots.extend(reg_subkey_values(HKEY_LOCAL_MACHINE, key, value).iter().filter_map(|p| as_root(p)));
    }
    roots
}

/// The string `value` of every direct subkey of `root\path` (missing keys / values are skipped).
fn reg_subkey_values(root: HKEY, path: &str, value: &str) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(root, &HSTRING::from(path), Some(0), KEY_READ, &mut key).is_err() {
            return out;
        }
        let value = HSTRING::from(value);
        for index in 0.. {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            let sub = PCWSTR(name.as_ptr());
            let mut size = 0u32;
            if RegGetValueW(key, sub, &value, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_err() || size < 2 {
                continue;
            }
            let mut buf = vec![0u16; (size as usize).div_ceil(2)];
            if RegGetValueW(key, sub, &value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
                .is_ok()
            {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                out.push(String::from_utf16_lossy(&buf[..end]));
            }
        }
        let _ = RegCloseKey(key);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_folders() {
        let lib = GameLibrary { install_roots: vec![as_root(r"D:\MyGames\Fortnite\").unwrap()], ..Default::default() };
        assert!(lib.is_launcher_install(
            r"F:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"
        ));
        assert!(lib.is_launcher_install(
            r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe"
        ));
        assert!(lib.is_launcher_install(r"C:\XboxGames\Forza Horizon 5\Content\ForzaHorizon5.exe"));
        assert!(lib
            .is_launcher_install(r"d:/mygames/fortnite/FortniteGame/Binaries/Win64/FortniteClient-Win64-Shipping.exe"));
        assert!(!lib.is_launcher_install(r"C:\Users\me\AppData\Local\Programs\ChatGPT\ChatGPT.exe"));
        assert!(!lib.is_launcher_install(r"C:\Users\me\AppData\Local\Discord\app-1.0\Discord.exe"));
        assert!(!lib.is_launcher_install(
            r"F:\SteamLibrary\steamapps\common\Steamworks Shared\_CommonRedist\DirectX\Jun2010\DXSETUP.exe"
        ));
    }
    #[test]
    fn windows_game_list_matches_full_paths_case_insensitively() {
        let lib = GameLibrary {
            windows_games: [normalize(r"C:\Games\Indie\Gnomium.exe")].into_iter().collect(),
            ..Default::default()
        };
        assert!(lib.is_windows_game(r"c:\games\indie\GNOMIUM.EXE"));
        assert!(!lib.is_windows_game(r"C:\Other\Gnomium.exe"));
    }
    #[test]
    fn epic_manifest_and_roots() {
        let item =
            r#"{ "FormatVersion": 0, "InstallLocation": "D:\\Epic Games\\Fortnite", "LaunchExecutable": "x.exe" }"#;
        assert_eq!(epic_install_location(item).as_deref(), Some(r"D:\Epic Games\Fortnite"));
        assert_eq!(epic_install_location(r#"{"Other": 1}"#), None);
        assert_eq!(as_root(r"D:\Epic Games\Fortnite").as_deref(), Some(r"d:\epic games\fortnite\"));
        assert_eq!(as_root("C:\\"), None, "a drive root would make everything a game");
        assert_eq!(as_root(""), None);
    }
    #[test]
    fn reads_this_machine_without_failing() {
        let mut lib = GameLibrary::default();
        lib.refresh_if_stale();
        let _ = lib.is_windows_game(r"C:\nothing.exe");
    }
}
