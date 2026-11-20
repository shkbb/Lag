//! The folder tree in the cloud (spec §2): `<root>/<game>/[<YYYY-MM>/]<file>` plus the system folder
//! `<root>/.lag/` (index + thumbnails). Paths are for people browsing their cloud; the app finds clips by
//! clip id, never by path, so a user may move or rename files there.
use crate::ids::ClipId;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

/// Lag's own folder inside the root: the index and thumbnails. Never a game folder.
pub const SYSTEM_FOLDER: &str = ".lag";
pub const INDEX_FILE: &str = "library.json";
pub const THUMBS_FOLDER: &str = "thumbs";
/// Clips recorded with no game (desktop capture).
pub const NO_GAME_FOLDER: &str = "Desktop";
pub const DEFAULT_ROOT: &str = "Lag";
/// Longest folder or file name kept (characters); the strictest provider allows 255 bytes.
const MAX_NAME: usize = 120;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortMode {
    /// `<root>/<game>/<file>`
    #[default]
    ByGame,
    /// `<root>/<game>/<YYYY-MM>/<file>`
    ByGameMonth,
    /// `<root>/<YYYY-MM>/<file>`
    ByMonth,
    /// `<root>/<file>`
    Flat,
}

/// Folder segments (root first) a clip goes to.
pub fn clip_folder(root: &str, mode: SortMode, game: Option<&str>, created: NaiveDateTime) -> Vec<String> {
    let mut path = vec![safe_name(root, DEFAULT_ROOT)];
    let game = || safe_name(game.unwrap_or(""), NO_GAME_FOLDER);
    let month = || created.format("%Y-%m").to_string();
    match mode {
        SortMode::ByGame => path.push(game()),
        SortMode::ByGameMonth => {
            path.push(game());
            path.push(month());
        }
        SortMode::ByMonth => path.push(month()),
        SortMode::Flat => {}
    }
    path
}

/// `<root>/.lag/library.json` as segments (the last one is the file).
pub fn index_path(root: &str) -> Vec<String> {
    vec![safe_name(root, DEFAULT_ROOT), SYSTEM_FOLDER.into(), INDEX_FILE.into()]
}

/// `<root>/.lag/thumbs/<clip id>.jpg` as segments.
pub fn thumbnail_path(root: &str, clip: ClipId) -> Vec<String> {
    vec![safe_name(root, DEFAULT_ROOT), SYSTEM_FOLDER.into(), THUMBS_FOLDER.into(), format!("{clip}.jpg")]
}

/// A folder or file name every provider accepts (one rule set, the strictest of them): no `<>:"/\|?*` or
/// control characters, no leading dots (hidden files, and no clash with `.lag`), no trailing dots or spaces,
/// no reserved Windows device names, at most 120 characters. Empty after cleaning -> `fallback`.
pub fn safe_name(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name: String = collapsed.trim_start_matches(['.', ' ']).chars().take(MAX_NAME).collect();
    name.truncate(name.trim_end_matches(['.', ' ']).len());
    if name.is_empty() {
        return if fallback.is_empty() { "_".into() } else { safe_name(fallback, "_") };
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        name.insert(0, '_');
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap()
    }

    #[test]
    fn sort_modes_build_the_tree() {
        let t = at(2026, 11, 20);
        assert_eq!(clip_folder("Lag", SortMode::ByGame, Some("Counter-Strike 2"), t), ["Lag", "Counter-Strike 2"]);
        assert_eq!(clip_folder("Lag", SortMode::ByGameMonth, Some("GTA V"), t), ["Lag", "GTA V", "2026-11"]);
        assert_eq!(clip_folder("Lag", SortMode::ByMonth, Some("GTA V"), t), ["Lag", "2026-11"]);
        assert_eq!(clip_folder("Lag", SortMode::Flat, Some("GTA V"), t), ["Lag"]);
        // no game, a blank game, an empty root
        assert_eq!(clip_folder("", SortMode::ByGame, None, t), ["Lag", "Desktop"]);
        assert_eq!(clip_folder("My clips", SortMode::ByGame, Some("  "), t), ["My clips", "Desktop"]);
        assert_eq!(index_path("Lag"), ["Lag", ".lag", "library.json"]);
    }

    #[test]
    fn names_are_safe_everywhere() {
        assert_eq!(safe_name("S.T.A.L.K.E.R.", "x"), "S.T.A.L.K.E.R");
        assert_eq!(safe_name("Tom Clancy's: Rainbow Six | Siege", "x"), "Tom Clancy's Rainbow Six Siege");
        assert_eq!(safe_name(r"a/b\c", "x"), "a b c");
        assert_eq!(safe_name(".lag", "x"), "lag", "never the system folder");
        assert_eq!(safe_name("...", "Desktop"), "Desktop");
        assert_eq!(safe_name("CON", "x"), "_CON");
        assert_eq!(safe_name("com1.game", "x"), "_com1.game");
        assert_eq!(safe_name("Command", "x"), "Command");
        assert_eq!(safe_name("Ведьмак 3: Дикая Охота", "x"), "Ведьмак 3 Дикая Охота");
        assert_eq!(safe_name(&"й".repeat(300), "x").chars().count(), 120);
        assert_eq!(safe_name("\u{7}\t", ""), "_");
    }
}
