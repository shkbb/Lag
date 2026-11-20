//! The user's cloud settings (spec §6, §7) and the one rule that follows from them: which accounts a clip
//! goes to. Stored in their own file `%APPDATA%\Lag\cloud.json` next to settings.json (that file stays
//! readable by the old C# build line by line; a nested object would break it). Tokens are never here (DPAPI
//! store, spec §9).
use crate::ids::{AccountId, ProviderKind};
use crate::layout::{safe_name, SortMode, DEFAULT_ROOT};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The choices of the cloud limit (GB), like the library's own storage limit.
pub const LIMIT_RUNGS_GB: &[u32] = &[10, 25, 50, 100, 250, 500, 1000, 2000];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Role {
    /// Gets every clip the upload rule selects; its index is shown first.
    #[default]
    Primary,
    /// A second copy (backup) of the selection, or of favorites only.
    Mirror,
    /// Connected and shown in the library, nothing new goes up.
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MirrorSelection {
    #[default]
    Everything,
    Favorites,
}

/// Which new clips go up by themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UploadWhat {
    /// Every saved replay.
    #[default]
    All,
    /// Only clips marked favorite (marking one later uploads it).
    Favorites,
    /// Nothing automatic: the "Upload" action only.
    Manual,
}

/// When queued uploads may run. Recording comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UploadWhen {
    Immediately,
    /// Paused while a game is running (the default: the upload never competes with the game).
    #[default]
    NoGameRunning,
    /// Only when the user has been away for a while.
    Idle,
}

/// What happens to the local file once a clip is safely in the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum KeepLocal {
    /// Keep it (the library size limit still applies, spec §11).
    #[default]
    Always,
    /// Free local space: remove local copies older than this many days (they stay in the cloud).
    FreeAfterDays(u32),
}

/// What "Delete" in the library does to a clip that is also in the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeleteMode {
    /// Ask every time (this PC only / everywhere).
    #[default]
    Ask,
    ThisPcOnly,
    Everywhere,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountSettings {
    pub id: AccountId,
    pub provider: ProviderKind,
    /// Shown in Settings (e-mail or name the provider reports).
    pub display_name: String,
    pub role: Role,
    pub mirror: MirrorSelection,
    /// Cloud auto-cleanup, the cloud's own "library size limit" (like Settings > Library): off by default;
    /// when on, Lag's clips in this account are kept within `max_size_gb` by removing the oldest
    /// non-favorite ones. A cloud full of other files never triggers it (quota.rs).
    pub auto_cleanup: bool,
    pub max_size_gb: u32,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            id: AccountId::new(ProviderKind::GoogleDrive, ""),
            provider: ProviderKind::GoogleDrive,
            display_name: String::new(),
            role: Role::Primary,
            mirror: MirrorSelection::Everything,
            auto_cleanup: false,
            max_size_gb: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CloudSettings {
    pub enabled: bool,
    pub root_folder: String,
    pub sort: SortMode,
    pub upload_what: UploadWhat,
    pub upload_when: UploadWhen,
    /// KB/s; 0 = no limit.
    pub upload_limit_kbps: u32,
    pub upload_screenshots: bool,
    pub keep_local: KeepLocal,
    pub delete_mode: DeleteMode,
    pub accounts: Vec<AccountSettings>,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            root_folder: DEFAULT_ROOT.into(),
            sort: SortMode::default(),
            upload_what: UploadWhat::default(),
            upload_when: UploadWhen::default(),
            upload_limit_kbps: 0,
            upload_screenshots: true,
            keep_local: KeepLocal::default(),
            delete_mode: DeleteMode::default(),
            accounts: Vec::new(),
        }
    }
}

/// What the rule needs to know about a clip.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClipFacts {
    pub favorite: bool,
    pub is_image: bool,
}

/// Why the question is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A replay was saved or a clip became a favorite.
    Automatic,
    /// The user chose "Upload" on the clip.
    Manual,
}

impl CloudSettings {
    /// The stored settings; defaults when the file is missing or unreadable (never an error: the cloud is
    /// simply off).
    pub fn load(path: &Path) -> Self {
        let mut settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(text.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default();
        settings.normalize();
        settings
    }

    /// Write-then-rename, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }

    /// Makes stored settings consistent: at most one primary (extra ones become mirrors; a lone mirror is
    /// promoted), a usable root folder name, no duplicate accounts.
    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.accounts.retain(|a| seen.insert(a.id.clone()));
        let mut primary = false;
        for a in &mut self.accounts {
            if a.role == Role::Primary {
                if primary {
                    a.role = Role::Mirror;
                }
                primary = true;
            }
        }
        if !primary {
            if let Some(a) = self.accounts.iter_mut().find(|a| a.role == Role::Mirror) {
                a.role = Role::Primary;
            }
        }
        self.root_folder = safe_name(&self.root_folder, DEFAULT_ROOT);
    }

    /// The accounts a clip goes to. Automatic uploads follow "what to upload" and the screenshots switch;
    /// a manual upload always goes to the primary and to every mirror whose selection includes the clip.
    pub fn targets(&self, clip: ClipFacts, trigger: Trigger) -> Vec<&AccountId> {
        if !self.enabled {
            return Vec::new();
        }
        if trigger == Trigger::Automatic {
            let selected = match self.upload_what {
                UploadWhat::All => true,
                UploadWhat::Favorites => clip.favorite,
                UploadWhat::Manual => false,
            };
            if !selected || (clip.is_image && !self.upload_screenshots) {
                return Vec::new();
            }
        }
        self.accounts
            .iter()
            .filter(|a| match a.role {
                Role::Primary => true,
                Role::Mirror => a.mirror == MirrorSelection::Everything || clip.favorite,
                Role::Paused => false,
            })
            .map(|a| &a.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str, role: Role, mirror: MirrorSelection) -> AccountSettings {
        AccountSettings {
            id: AccountId::new(ProviderKind::GoogleDrive, id),
            role,
            mirror,
            ..Default::default()
        }
    }

    fn settings(what: UploadWhat) -> CloudSettings {
        CloudSettings {
            enabled: true,
            upload_what: what,
            accounts: vec![
                account("main", Role::Primary, MirrorSelection::Everything),
                account("backup", Role::Mirror, MirrorSelection::Favorites),
                account("old", Role::Paused, MirrorSelection::Everything),
            ],
            ..Default::default()
        }
    }

    fn names(ids: Vec<&AccountId>) -> Vec<String> {
        ids.iter().map(|a| a.as_str().trim_start_matches("gdrive:").to_owned()).collect()
    }

    #[test]
    fn clips_go_where_the_roles_say() {
        let s = settings(UploadWhat::All);
        let plain = ClipFacts::default();
        let fav = ClipFacts { favorite: true, ..Default::default() };
        assert_eq!(names(s.targets(plain, Trigger::Automatic)), ["main"]);
        assert_eq!(names(s.targets(fav, Trigger::Automatic)), ["main", "backup"], "favorites mirror");
        // screenshots go up by default (owner, 2026-11-20); switched off, only a manual upload sends them
        let shot = ClipFacts { is_image: true, ..Default::default() };
        assert_eq!(names(s.targets(shot, Trigger::Automatic)), ["main"]);
        let no_shots = CloudSettings { upload_screenshots: false, ..s.clone() };
        assert!(no_shots.targets(shot, Trigger::Automatic).is_empty());
        assert_eq!(names(no_shots.targets(shot, Trigger::Manual)), ["main"]);
    }

    #[test]
    fn what_to_upload_and_off_switch() {
        let fav = ClipFacts { favorite: true, ..Default::default() };
        let s = settings(UploadWhat::Favorites);
        assert!(s.targets(ClipFacts::default(), Trigger::Automatic).is_empty());
        assert_eq!(names(s.targets(fav, Trigger::Automatic)), ["main", "backup"]);
        let s = settings(UploadWhat::Manual);
        assert!(s.targets(fav, Trigger::Automatic).is_empty());
        assert_eq!(names(s.targets(fav, Trigger::Manual)), ["main", "backup"]);
        let off = CloudSettings { enabled: false, ..settings(UploadWhat::All) };
        assert!(off.targets(fav, Trigger::Manual).is_empty());
    }

    #[test]
    fn normalize_keeps_one_primary_and_a_safe_root() {
        let mut s = CloudSettings {
            root_folder: "  ../Clips:  ".into(),
            accounts: vec![
                account("a", Role::Primary, MirrorSelection::Everything),
                account("b", Role::Primary, MirrorSelection::Everything),
                account("a", Role::Mirror, MirrorSelection::Everything),
            ],
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.accounts.len(), 2, "duplicate account dropped");
        assert_eq!((s.accounts[0].role, s.accounts[1].role), (Role::Primary, Role::Mirror));
        assert_eq!(s.root_folder, "Clips");
        let mut lone = CloudSettings { accounts: vec![account("m", Role::Mirror, MirrorSelection::Favorites)], ..Default::default() };
        lone.normalize();
        assert_eq!(lone.accounts[0].role, Role::Primary);
    }

    #[test]
    fn settings_round_trip_as_pascal_case() {
        let s = settings(UploadWhat::Favorites);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"UploadWhat\":\"Favorites\""), "{json}");
        assert_eq!(serde_json::from_str::<CloudSettings>(&json).unwrap(), s);
        assert_eq!(serde_json::from_str::<CloudSettings>("{}").unwrap(), CloudSettings::default());
    }

    #[test]
    fn the_file_is_saved_and_a_bad_one_means_defaults() {
        let dir = std::env::temp_dir().join(format!("lag-cloud-settings-{}", std::process::id()));
        let path = dir.join("cloud.json");
        assert_eq!(CloudSettings::load(&path), CloudSettings::default(), "no file: cloud off");
        let s = settings(UploadWhat::Favorites);
        s.save(&path).unwrap();
        assert_eq!(CloudSettings::load(&path), s);
        std::fs::write(&path, "{ broken").unwrap();
        assert_eq!(CloudSettings::load(&path), CloudSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
