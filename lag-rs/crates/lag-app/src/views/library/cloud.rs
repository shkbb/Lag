//! The library with the cloud (docs/spec/cloud.md §5, §11, §12): local clips joined with every connected
//! account's index, so each card knows where it lives, and clips that are only in the cloud appear as cards of
//! their own (keys `cloud:<id>/<name>`, played from the cloud). Pure where it can be: the joining itself is
//! `lag_cloud::presence::join_library`.
use super::clip::ClipEntry;
use crate::services::cloud::{stream, thumbnail_path, CloudLibrary};
use chrono::Local;
use lag_cloud::index::IndexEntry;
use lag_cloud::presence::{join_library, Badge, LocalClip};
use lag_cloud::ClipId;
use lag_core::metadata::ClipMetadataStore;
use std::collections::HashMap;

/// Where a card's clip lives, as the card shows it (LibraryCardData.cloud-state / cloud-progress).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CloudMark {
    /// 0 only here, 1 here + cloud, 2 only in the cloud, 3 uploading / queued, 4 downloading, 5 failed.
    pub state: i32,
    /// 0..1 while moving; below 0 = waiting in the queue.
    pub progress: f32,
}

pub(super) const SYNCED: i32 = 1;
pub(super) const CLOUD_ONLY: i32 = 2;

fn mark(badge: &Badge, cloud_only: bool) -> CloudMark {
    match badge {
        Badge::LocalOnly => CloudMark::default(),
        Badge::Synced => CloudMark { state: SYNCED, progress: 0.0 },
        Badge::CloudOnly => CloudMark { state: CLOUD_ONLY, progress: 0.0 },
        Badge::Busy(p) => CloudMark { state: if cloud_only { 4 } else { 3 }, progress: p.unwrap_or(-1.0) },
        Badge::Failed(_) => CloudMark { state: 5, progress: 0.0 },
    }
}

/// A card for a clip that is only in the cloud.
fn cloud_entry(id: ClipId, e: &IndexEntry) -> ClipEntry {
    let key = stream::key(id, &e.file_name);
    ClipEntry {
        is_favorite: e.favorite,
        game: e.game.clone(),
        exe: e.exe.clone(),
        is_edited: e.edited,
        is_image: e.is_image,
        thumbnail_path: thumbnail_path(&key).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        file_path: key,
        duration_ticks: e.duration_seconds.map_or(0, |s| (s * 10_000_000.0).round() as i64),
        created_date: e.created.with_timezone(&Local).naive_local(),
        file_size: e.size as i64,
        cloud_id: Some(id),
        ..Default::default()
    }
}

/// Joins `clips` (the local scan, plus cloud cards from before) with the cloud: marks every local card, adds
/// or drops the cloud-only cards. A local clip that matched a cloud clip by name + size + duration (its
/// metadata was lost) gets the cloud's id written back. Returns whether the set of cloud-only cards changed
/// (then the view needs a full rebuild, otherwise refreshing the visible cards is enough).
pub(super) fn merge(clips: &mut Vec<ClipEntry>, cloud: &CloudLibrary, store: Option<&ClipMetadataStore>) -> bool {
    let before: Vec<String> = clips.iter().filter(|c| c.is_cloud_only()).map(|c| c.file_path.clone()).collect();
    let mut old_cloud: HashMap<String, ClipEntry> =
        clips.iter().filter(|c| c.is_cloud_only()).map(|c| (c.file_path.clone(), c.clone())).collect();
    clips.retain(|c| !c.is_cloud_only());
    for c in clips.iter_mut() {
        c.cloud = CloudMark::default();
    }
    if !cloud.connected {
        return !before.is_empty();
    }

    let local: Vec<LocalClip> = clips
        .iter()
        .map(|c| LocalClip {
            id: c.cloud_id,
            path: c.file_path.clone(),
            file_name: std::path::Path::new(&c.file_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            size: c.file_size.max(0) as u64,
            duration_seconds: (c.duration_ticks > 0).then(|| c.duration_ticks as f64 / 10_000_000.0),
        })
        .collect();
    let indexes: Vec<_> = cloud.indexes.iter().map(|(a, i)| (a.clone(), i)).collect();
    let mut added = Vec::new();
    for item in join_library(&local, &indexes, &cloud.transfers) {
        let badge = item.presence.badge();
        match item.local {
            Some(i) => {
                clips[i].cloud = mark(&badge, false);
                if let (true, Some(id)) = (item.adopt_id, item.id) {
                    clips[i].cloud_id = Some(id);
                    if let Some(store) = store {
                        store.set_cloud_id(&clips[i].file_path, &id.to_string());
                    }
                }
            }
            None => {
                if let (Some(id), Some(e)) = (item.id, &item.cloud) {
                    let key = stream::key(id, &e.file_name);
                    let mut entry = old_cloud.remove(&key).unwrap_or_else(|| cloud_entry(id, e));
                    entry.is_favorite = e.favorite;
                    entry.cloud = mark(&badge, true);
                    added.push(entry);
                }
            }
        }
    }
    if !added.is_empty() {
        clips.extend(added);
        clips.sort_by(|a, b| b.created_date.cmp(&a.created_date));
    }
    let mut after: Vec<String> = clips.iter().filter(|c| c.is_cloud_only()).map(|c| c.file_path.clone()).collect();
    let mut before = before;
    before.sort();
    after.sort();
    before != after
}

/// What deleting a set of clips involves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct DeleteScope {
    /// Clips only on this PC.
    pub local_only: usize,
    /// Clips here and in the cloud.
    pub both: usize,
    /// Clips only in the cloud.
    pub cloud_only: usize,
}

impl DeleteScope {
    pub(super) fn of<'a>(clips: impl IntoIterator<Item = &'a ClipEntry>) -> Self {
        let mut s = Self::default();
        for c in clips {
            match c.cloud.state {
                CLOUD_ONLY => s.cloud_only += 1,
                SYNCED => s.both += 1,
                _ if c.is_cloud_only() => s.cloud_only += 1,
                _ => s.local_only += 1,
            }
        }
        s
    }
    /// Nothing in the cloud: delete right away, as before the cloud.
    pub(super) fn local(&self) -> bool {
        self.both == 0 && self.cloud_only == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lag_cloud::index::CloudIndex;
    use lag_cloud::presence::Transfer;
    use lag_cloud::{AccountId, ProviderKind};

    fn local(path: &str, size: i64, id: Option<ClipId>) -> ClipEntry {
        ClipEntry { file_path: path.into(), file_size: size, cloud_id: id, duration_ticks: 1_200_000_000, ..Default::default() }
    }

    #[test]
    fn local_clips_get_marks_and_cloud_clips_get_cards() {
        let drive = AccountId::new(ProviderKind::GoogleDrive, "1");
        let (synced, only, uploading) = (ClipId::new_random(), ClipId::new_random(), ClipId::new_random());
        let mut index = CloudIndex::default();
        let entry = |name: &str| IndexEntry {
            file_name: name.into(),
            size: 10,
            duration_seconds: Some(120.0),
            remote_id: Some("r".into()),
            ..Default::default()
        };
        index.upsert(synced, entry("a.mp4"), chrono::Utc::now());
        index.upsert(only, IndexEntry { favorite: true, game: Some("CS2".into()), ..entry("b.mp4") }, chrono::Utc::now());
        let mut transfers = HashMap::new();
        transfers.insert((uploading, drive.clone()), Transfer::Uploading(0.5));
        let cloud = CloudLibrary { indexes: vec![(drive, index)], transfers, connected: true };

        let mut clips = vec![
            local(r"D:\Lag\a.mp4", 10, Some(synced)),
            local(r"D:\Lag\c.mp4", 10, Some(uploading)),
            local(r"D:\Lag\d.mp4", 10, None),
        ];
        assert!(merge(&mut clips, &cloud, None), "a cloud-only card appeared");
        let by = |p: &str| clips.iter().find(|c| c.file_path.contains(p)).unwrap().clone();
        assert_eq!(by("a.mp4").cloud.state, SYNCED);
        assert_eq!(by("c.mp4").cloud, CloudMark { state: 3, progress: 0.5 });
        assert_eq!(by("d.mp4").cloud.state, 0);
        let b = by("b.mp4");
        assert!(b.is_cloud_only() && b.is_favorite && b.game.as_deref() == Some("CS2"));
        assert_eq!((b.cloud.state, b.duration_ticks), (CLOUD_ONLY, 1_200_000_000));

        // the same again: nothing new to lay out, only marks
        assert!(!merge(&mut clips, &cloud, None));
        assert_eq!(clips.len(), 4);
        // disconnected: the cloud cards go, the marks too
        assert!(merge(&mut clips, &CloudLibrary::default(), None));
        assert_eq!(clips.len(), 3);
        assert!(clips.iter().all(|c| c.cloud.state == 0));
    }

    #[test]
    fn delete_scope_tells_what_to_ask() {
        let mut a = local("a", 1, None);
        let mut b = local("b", 1, None);
        b.cloud.state = SYNCED;
        let mut c = local(&stream::key(ClipId::new_random(), "c.mp4"), 1, None);
        c.cloud.state = CLOUD_ONLY;
        assert!(DeleteScope::of([&a]).local());
        assert_eq!(DeleteScope::of([&a, &b, &c]), DeleteScope { local_only: 1, both: 1, cloud_only: 1 });
        a.cloud.state = 3; // still uploading: it is not in the cloud yet
        assert!(DeleteScope::of([&a]).local());
    }
}
