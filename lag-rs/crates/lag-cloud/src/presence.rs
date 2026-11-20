//! Where a clip lives (spec §5) and the library a user sees: the local scan joined with every connected
//! account's index, one item per clip. This is what makes a new PC show the whole library after connecting
//! its cloud, and what keeps a downloaded clip from appearing twice.
use crate::ids::{AccountId, ClipId};
use crate::index::{CloudIndex, IndexEntry};
use std::collections::{BTreeMap, HashMap};

/// Work in flight (or failed) for one clip in one account.
#[derive(Debug, Clone, PartialEq)]
pub enum Transfer {
    Queued,
    /// 0..=1
    Uploading(f32),
    Downloading(f32),
    Failed(String),
}

/// One clip's state in one account.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InAccount {
    /// The account's index lists the clip (it is stored there).
    pub stored: bool,
    pub transfer: Option<Transfer>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Presence {
    /// The video file is on this PC.
    pub local: bool,
    pub accounts: BTreeMap<AccountId, InAccount>,
}

/// What the library card shows (the best state over all accounts; the tooltip lists each account).
#[derive(Debug, Clone, PartialEq)]
pub enum Badge {
    /// Only on this PC: no badge.
    LocalOnly,
    /// Going up or down; the progress of the slowest transfer, None while only queued.
    Busy(Option<f32>),
    /// Here and in at least one cloud.
    Synced,
    /// In the cloud only: playing it downloads (or streams) it.
    CloudOnly,
    /// The last transfer failed and the clip is in no cloud yet.
    Failed(String),
}

impl Presence {
    pub fn in_cloud(&self) -> bool {
        self.accounts.values().any(|a| a.stored)
    }

    pub fn badge(&self) -> Badge {
        let transfers: Vec<&Transfer> = self.accounts.values().filter_map(|a| a.transfer.as_ref()).collect();
        let progress: Vec<f32> = transfers
            .iter()
            .filter_map(|t| match t {
                Transfer::Uploading(p) | Transfer::Downloading(p) => Some(*p),
                _ => None,
            })
            .collect();
        if !progress.is_empty() {
            return Badge::Busy(progress.into_iter().reduce(f32::min));
        }
        if transfers.iter().any(|t| matches!(t, Transfer::Queued)) {
            return Badge::Busy(None);
        }
        match (self.local, self.in_cloud()) {
            (true, true) => Badge::Synced,
            (false, true) => Badge::CloudOnly,
            _ => match transfers.iter().find_map(|t| if let Transfer::Failed(r) = t { Some(r) } else { None }) {
                Some(reason) => Badge::Failed(reason.clone()),
                None => Badge::LocalOnly,
            },
        }
    }
}

/// A clip found on this PC by the library scan.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalClip {
    /// From the clip's metadata; None for clips that never met the cloud.
    pub id: Option<ClipId>,
    pub path: String,
    pub file_name: String,
    pub size: u64,
    pub duration_seconds: Option<f64>,
}

/// One library item: a local file, a cloud entry, or both.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryItem {
    pub id: Option<ClipId>,
    /// Index into the `local` slice given to [`join_library`].
    pub local: Option<usize>,
    /// The cloud's view of the clip (from the first account that has it): game, date, favorite, size...
    pub cloud: Option<IndexEntry>,
    pub presence: Presence,
    /// A local clip without an id that matched a cloud entry (name + size + duration): the caller writes
    /// this id into the clip's metadata so it is matched by id from now on.
    pub adopt_id: bool,
}

/// Durations closer than this are the same clip (metadata rounding between PCs).
const SAME_DURATION: f64 = 0.5;

/// The library: every local clip, plus every clip the accounts hold that is not on this PC. `indexes` are
/// the connected accounts in priority order (primary first); `transfers` is the queue's state per clip.
pub fn join_library(
    local: &[LocalClip],
    indexes: &[(AccountId, &CloudIndex)],
    transfers: &HashMap<(ClipId, AccountId), Transfer>,
) -> Vec<LibraryItem> {
    let mut items: Vec<LibraryItem> = Vec::new();
    let mut by_id: HashMap<ClipId, usize> = HashMap::new();

    for (i, clip) in local.iter().enumerate() {
        let item = LibraryItem {
            id: clip.id,
            local: Some(i),
            cloud: None,
            presence: Presence { local: true, accounts: BTreeMap::new() },
            adopt_id: false,
        };
        if let Some(id) = clip.id {
            by_id.insert(id, items.len());
        }
        items.push(item);
    }

    for (account, index) in indexes {
        for (id, entry) in index.live() {
            let slot = match by_id.get(id) {
                Some(&slot) => slot,
                None => match find_unmatched_local(&items, local, entry) {
                    // the same file without an id (reinstall, metadata wiped): adopt the cloud's id
                    Some(slot) => {
                        items[slot].id = Some(*id);
                        items[slot].adopt_id = true;
                        by_id.insert(*id, slot);
                        slot
                    }
                    None => {
                        items.push(LibraryItem {
                            id: Some(*id),
                            local: None,
                            cloud: None,
                            presence: Presence::default(),
                            adopt_id: false,
                        });
                        by_id.insert(*id, items.len() - 1);
                        items.len() - 1
                    }
                },
            };
            let item = &mut items[slot];
            item.presence.accounts.entry(account.clone()).or_default().stored = true;
            if item.cloud.is_none() {
                item.cloud = Some(entry.clone());
            }
        }
    }

    for ((id, account), transfer) in transfers {
        if let Some(&slot) = by_id.get(id) {
            items[slot].presence.accounts.entry(account.clone()).or_default().transfer = Some(transfer.clone());
        }
    }
    items
}

fn find_unmatched_local(items: &[LibraryItem], local: &[LocalClip], entry: &IndexEntry) -> Option<usize> {
    items.iter().position(|item| {
        item.id.is_none()
            && item.local.is_some_and(|i| {
                let clip = &local[i];
                clip.file_name.eq_ignore_ascii_case(&entry.file_name)
                    && clip.size == entry.size
                    && match (clip.duration_seconds, entry.duration_seconds) {
                        (Some(a), Some(b)) => (a - b).abs() < SAME_DURATION,
                        _ => true,
                    }
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProviderKind;
    use chrono::Utc;

    fn local(id: Option<ClipId>, name: &str, size: u64) -> LocalClip {
        LocalClip { id, path: format!(r"D:\Lag\{name}"), file_name: name.into(), size, duration_seconds: Some(120.0) }
    }
    fn entry(name: &str, size: u64) -> IndexEntry {
        IndexEntry { file_name: name.into(), size, duration_seconds: Some(120.2), ..Default::default() }
    }

    #[test]
    fn a_new_pc_sees_the_whole_cloud_library() {
        let drive = AccountId::new(ProviderKind::GoogleDrive, "1");
        let mut index = CloudIndex::default();
        for i in 0..3 {
            index.upsert(ClipId::new_random(), entry(&format!("{i}.mp4"), 10), Utc::now());
        }
        let items = join_library(&[], &[(drive.clone(), &index)], &HashMap::new());
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.presence.badge() == Badge::CloudOnly && i.cloud.is_some()));
    }

    #[test]
    fn a_downloaded_or_reinstalled_clip_is_never_shown_twice() {
        let drive = AccountId::new(ProviderKind::GoogleDrive, "1");
        let (known, lost) = (ClipId::new_random(), ClipId::new_random());
        let mut index = CloudIndex::default();
        index.upsert(known, entry("a.mp4", 10), Utc::now());
        index.upsert(lost, entry("b.mp4", 20), Utc::now());
        // a.mp4 carries its id; b.mp4 lost its metadata (same name, size, duration)
        let here = [local(Some(known), "a.mp4", 10), local(None, "b.mp4", 20), local(None, "c.mp4", 30)];
        let items = join_library(&here, &[(drive, &index)], &HashMap::new());
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].presence.badge(), Badge::Synced);
        assert_eq!((items[1].id, items[1].adopt_id, items[1].presence.badge()), (Some(lost), true, Badge::Synced));
        assert_eq!(items[2].presence.badge(), Badge::LocalOnly);
        // same name but another size is another clip
        let other = [local(None, "b.mp4", 21)];
        let mut index2 = CloudIndex::default();
        index2.upsert(lost, entry("b.mp4", 20), Utc::now());
        let items = join_library(&other, &[(AccountId::new(ProviderKind::Dropbox, "x"), &index2)], &HashMap::new());
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn badges_follow_transfers_across_accounts() {
        let (drive, dropbox) = (AccountId::new(ProviderKind::GoogleDrive, "1"), AccountId::new(ProviderKind::Dropbox, "2"));
        let id = ClipId::new_random();
        let mut index = CloudIndex::default();
        index.upsert(id, entry("a.mp4", 10), Utc::now());
        let here = [local(Some(id), "a.mp4", 10)];
        let mut transfers = HashMap::new();
        transfers.insert((id, dropbox.clone()), Transfer::Uploading(0.4));
        let items = join_library(&here, &[(drive.clone(), &index)], &transfers);
        assert_eq!(items[0].presence.badge(), Badge::Busy(Some(0.4)), "a mirror still uploading shows progress");
        assert!(items[0].presence.accounts[&drive].stored);

        let fresh = ClipId::new_random();
        let mut transfers = HashMap::new();
        transfers.insert((fresh, drive.clone()), Transfer::Failed("cloud is full".into()));
        let items = join_library(&[local(Some(fresh), "n.mp4", 1)], &[(drive, &CloudIndex::default())], &transfers);
        assert_eq!(items[0].presence.badge(), Badge::Failed("cloud is full".into()));
    }
}
