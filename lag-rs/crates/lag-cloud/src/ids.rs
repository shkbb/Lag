//! Identities (spec §1, §3). A clip's id never depends on its path: the same clip has the same id on every
//! PC, which is what lets a new PC recognise its downloads instead of showing duplicates.
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Stable id of one clip, written into its local metadata the first time it meets the cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipId(Uuid);

impl ClipId {
    pub fn new_random() -> Self {
        Self(Uuid::new_v4())
    }
    pub fn parse(text: &str) -> Option<Self> {
        Uuid::parse_str(text.trim()).ok().map(Self)
    }
}

impl fmt::Display for ClipId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

/// The clouds Lag can talk to. New ones are added at the end (the name is what settings store).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    GoogleDrive,
    Dropbox,
    OneDrive,
    /// A folder: a NAS share (`\\nas\share`), a mapped network drive, a synced folder (Syncthing...).
    Folder,
    /// A WebDAV server: Nextcloud, ownCloud, Synology, QNAP, TrueNAS, Seafile...
    WebDav,
}

impl ProviderKind {
    fn tag(self) -> &'static str {
        match self {
            Self::GoogleDrive => "gdrive",
            Self::Dropbox => "dropbox",
            Self::OneDrive => "onedrive",
            Self::Folder => "folder",
            Self::WebDav => "webdav",
        }
    }
}

/// One connected cloud: provider + the provider's own (stable) user id, e.g. `gdrive:1234567890`. The same
/// Google account connected on two PCs is the same account id on both.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(provider: ProviderKind, provider_user_id: &str) -> Self {
        Self(format!("{}:{}", provider.tag(), provider_user_id.trim()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clip_ids_are_unique_and_round_trip() {
        let a = ClipId::new_random();
        assert_ne!(a, ClipId::new_random());
        assert_eq!(ClipId::parse(&a.to_string()), Some(a));
        assert_eq!(serde_json::from_str::<ClipId>(&serde_json::to_string(&a).unwrap()).unwrap(), a);
        assert_eq!(ClipId::parse("not an id"), None);
    }
    #[test]
    fn account_ids_name_the_provider() {
        assert_eq!(AccountId::new(ProviderKind::GoogleDrive, " 42 ").as_str(), "gdrive:42");
        assert_ne!(AccountId::new(ProviderKind::Dropbox, "42"), AccountId::new(ProviderKind::OneDrive, "42"));
    }
}
