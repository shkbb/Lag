//! Storage limits with the cloud (spec §11). Plans only (pure); the library carries them out.
//!
//! Local library size limit (Settings > "Library size limit", auto-cleanup) keeps its meaning: the library
//! folder never grows past it, oldest videos first, screenshots never counted. With a cloud it first frees
//! space WITHOUT losing anything: a clip that is safely in a cloud only loses its local copy (it stays in the
//! library as "in the cloud"). Only if that is not enough are local-only clips deleted, as before. A clip
//! that is being uploaded or downloaded is never touched.
//!
//! Cloud auto-cleanup (per account, owner 2026-11-20: "the cloud needs a limit too"): when a clip does not fit
//! (the user's own limit for Lag in that cloud, or the cloud itself is full), either the oldest non-favorite
//! clips are removed from that cloud (default, like the local cleanup), or new uploads wait.
use crate::ids::ClipId;
use crate::index::CloudIndex;
use crate::settings::WhenFull;

/// One local file as the limit sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalFile {
    pub path: String,
    pub size: u64,
    /// Sort key, older = smaller (creation time ticks).
    pub created: i64,
    pub is_image: bool,
    /// Stored in at least one cloud.
    pub in_cloud: bool,
    /// An upload or download of it is in flight.
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAction {
    /// Remove the local file only; the clip stays in the library from the cloud.
    Evict(String),
    /// Remove the clip (it is nowhere else): today's auto-cleanup.
    Delete(String),
}

/// What to remove so the library's videos fit in `max_bytes` (0 = no limit).
pub fn plan_local_limit(files: &[LocalFile], max_bytes: u64) -> Vec<LocalAction> {
    if max_bytes == 0 {
        return Vec::new();
    }
    let mut videos: Vec<&LocalFile> = files.iter().filter(|f| !f.is_image).collect();
    let mut total: u64 = videos.iter().map(|f| f.size).sum();
    if total <= max_bytes {
        return Vec::new();
    }
    videos.sort_by_key(|f| f.created);
    let mut plan = Vec::new();
    // lossless first, then the old behaviour
    for in_cloud in [true, false] {
        for f in videos.iter().filter(|f| f.in_cloud == in_cloud && !f.busy) {
            if total <= max_bytes {
                return plan;
            }
            total -= f.size;
            plan.push(if in_cloud { LocalAction::Evict(f.path.clone()) } else { LocalAction::Delete(f.path.clone()) });
        }
    }
    plan
}

/// "Free local space after N days": local copies of clips older than `cutoff` that are safely in a cloud.
pub fn plan_free_space(files: &[LocalFile], cutoff: i64) -> Vec<LocalAction> {
    files
        .iter()
        .filter(|f| f.in_cloud && !f.busy && f.created < cutoff)
        .map(|f| LocalAction::Evict(f.path.clone()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudLimitPlan {
    /// Clips to remove from this cloud, oldest first, never favorites (auto-cleanup).
    pub delete: Vec<ClipId>,
    /// Still does not fit after that (or cleanup is off): uploads to this account wait ("cloud is full").
    pub full: bool,
}

/// The provider's own storage, as it reports it (Drive `about.storageQuota`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpace {
    /// Free bytes in the whole account (other files of the user count against it too).
    pub free: u64,
}

const GIB: u64 = 1 << 30;

/// The cloud's auto-cleanup: make room for `incoming` bytes in one account. The room Lag may use is the
/// smaller of the user's own limit (`max_size_gb`, 0 = none) and what the cloud itself has left; so cleanup
/// also runs when the user's Google Drive fills up with other files, not only at Lag's own limit.
pub fn plan_cloud_limit(
    index: &CloudIndex,
    max_size_gb: u32,
    provider: Option<ProviderSpace>,
    when_full: WhenFull,
    incoming: u64,
) -> CloudLimitPlan {
    let used = index.live_bytes();
    let own = (max_size_gb > 0).then(|| u64::from(max_size_gb) * GIB);
    let cloud = provider.map(|p| used + p.free);
    let Some(limit) = own.into_iter().chain(cloud).min() else {
        return CloudLimitPlan::default();
    };
    let mut total = used + incoming;
    if total <= limit {
        return CloudLimitPlan::default();
    }
    let mut plan = CloudLimitPlan::default();
    if when_full == WhenFull::RemoveOldest {
        let mut oldest: Vec<_> = index.live().filter(|(_, e)| !e.favorite).collect();
        oldest.sort_by_key(|(_, e)| e.created);
        for (id, e) in oldest {
            if total <= limit {
                break;
            }
            total -= e.size;
            plan.delete.push(*id);
        }
    }
    plan.full = total > limit;
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexEntry;
    use chrono::{TimeZone, Utc};

    fn file(path: &str, size: u64, created: i64, in_cloud: bool) -> LocalFile {
        LocalFile { path: path.into(), size, created, is_image: false, in_cloud, busy: false }
    }

    #[test]
    fn the_local_limit_frees_cloud_copies_before_deleting_anything() {
        let files = [
            file("old-local", 10, 1, false),
            file("old-cloud", 10, 2, true),
            file("new-cloud", 10, 3, true),
            file("newest", 10, 4, false),
            LocalFile { is_image: true, ..file("shot.png", 100, 0, false) },
        ];
        // 40 bytes of video, limit 25: two cloud copies go, nothing is lost
        assert_eq!(
            plan_local_limit(&files, 25),
            [LocalAction::Evict("old-cloud".into()), LocalAction::Evict("new-cloud".into())]
        );
        // limit 15: then the oldest local-only clip is deleted, as before the cloud
        assert_eq!(plan_local_limit(&files, 15).last(), Some(&LocalAction::Delete("old-local".into())));
        assert!(plan_local_limit(&files, 40).is_empty());
        assert!(plan_local_limit(&files, 0).is_empty(), "no limit");
    }

    #[test]
    fn clips_in_flight_are_never_touched() {
        let files = [LocalFile { busy: true, ..file("uploading", 10, 1, false) }, file("other", 10, 2, false)];
        assert_eq!(plan_local_limit(&files, 5), [LocalAction::Delete("other".into())]);
        let files = [LocalFile { busy: true, ..file("downloading", 10, 1, true) }, file("kept", 10, 9, true)];
        assert_eq!(plan_free_space(&files, 5), Vec::<LocalAction>::new());
        assert_eq!(plan_free_space(&files, 10), [LocalAction::Evict("kept".into())]);
    }

    #[test]
    fn the_cloud_auto_cleanup_frees_room_or_stops_uploads() {
        // 3 clips of 1 GB in the cloud, the oldest one a favorite
        let mut index = CloudIndex::default();
        let mut ids = Vec::new();
        for (i, favorite) in [(1, true), (2, false), (3, false)] {
            let id = ClipId::new_random();
            let created = Utc.timestamp_opt(1_790_000_000 + i, 0).unwrap();
            index.upsert(id, IndexEntry { size: GIB, created, favorite, ..Default::default() }, Utc::now());
            ids.push(id);
        }
        let remove = WhenFull::RemoveOldest;
        // limit 3 GB, 1 GB coming: the oldest NON-favorite goes
        let plan = plan_cloud_limit(&index, 3, None, remove, GIB);
        assert_eq!((plan.delete, plan.full), (vec![ids[1]], false));
        // favorites are never removed: limit 1 GB cannot be met
        let plan = plan_cloud_limit(&index, 1, None, remove, GIB);
        assert_eq!((plan.delete.len(), plan.full), (2, true));
        // "stop uploads": nothing removed, the cloud is full
        assert_eq!(plan_cloud_limit(&index, 3, None, WhenFull::StopUploads, GIB), CloudLimitPlan { delete: vec![], full: true });
        // no limit of our own and the cloud has room: nothing to do
        assert_eq!(plan_cloud_limit(&index, 0, Some(ProviderSpace { free: 5 * GIB }), remove, GIB), CloudLimitPlan::default());
        // no own limit, but the user's Drive itself is full (other files): cleanup still makes room
        let plan = plan_cloud_limit(&index, 0, Some(ProviderSpace { free: GIB / 2 }), remove, GIB);
        assert_eq!((plan.delete, plan.full), (vec![ids[1]], false));
        // the smaller of the two limits wins
        let plan = plan_cloud_limit(&index, 100, Some(ProviderSpace { free: 0 }), remove, GIB);
        assert_eq!(plan.delete, [ids[1]]);
    }
}
