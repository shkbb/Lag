//! Storage limits with the cloud (spec §11). Plans only (pure); the library carries them out.
//!
//! Local library size limit (Settings > "Library size limit", auto-cleanup) keeps its meaning: the library
//! folder never grows past it, oldest videos first, screenshots never counted. With a cloud it first frees
//! space WITHOUT losing anything: a clip that is safely in a cloud only loses its local copy (it stays in the
//! library as "in the cloud"). Only if that is not enough are local-only clips deleted, as before. A clip
//! that is being uploaded or downloaded is never touched.
//!
//! Cloud auto-cleanup (per account, owner 2026-11-20: "the cloud needs a limit too") works exactly like the
//! local one: OFF unless the user switches it on, and then only against the user's own limit for Lag in that
//! cloud (oldest first, never favorites). A cloud that is full for any other reason (the user's own files on
//! Google Drive) never makes Lag delete anything: the upload fails with "not enough space in <cloud>", and
//! Settings shows how much room each cloud has left.
use crate::ids::ClipId;
use crate::index::CloudIndex;

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

/// Why a clip cannot go up right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoRoom {
    /// Over the user's own limit for Lag in this cloud, and auto-cleanup cannot (or may not) free enough.
    OwnLimit,
    /// The cloud itself has no space left (Lag never deletes anything for this).
    CloudFull,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudLimitPlan {
    /// Clips the auto-cleanup removes from this cloud first: oldest, never favorites.
    pub delete: Vec<ClipId>,
    /// Set = the clip does not fit even after `delete`: the upload fails with this reason.
    pub no_room: Option<NoRoom>,
}

/// The provider's own storage, as it reports it (Drive `about.storageQuota`). Shown in Settings per cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSpace {
    /// Free bytes in the whole account (the user's other files count against it too).
    pub free: u64,
    /// Size of the account's storage; None = unlimited (some business plans).
    pub total: Option<u64>,
}

pub const GIB: u64 = 1 << 30;

/// Room for `incoming` bytes in one account. Auto-cleanup (`auto_cleanup` + `max_size_gb`, the cloud's
/// "library size limit") removes the oldest non-favorite clips until Lag's clips fit the user's limit; the
/// cloud's own free space is only CHECKED, never cleaned for.
pub fn plan_cloud_limit(
    index: &CloudIndex,
    auto_cleanup: bool,
    max_size_gb: u32,
    provider: Option<ProviderSpace>,
    incoming: u64,
) -> CloudLimitPlan {
    let mut plan = CloudLimitPlan::default();
    let mut freed = 0u64;
    if auto_cleanup && max_size_gb > 0 {
        let limit = u64::from(max_size_gb) * GIB;
        let mut total = index.live_bytes() + incoming;
        let mut oldest: Vec<_> = index.live().filter(|(_, e)| !e.favorite).collect();
        oldest.sort_by_key(|(_, e)| e.created);
        for (id, e) in oldest {
            if total <= limit {
                break;
            }
            total -= e.size;
            freed += e.size;
            plan.delete.push(*id);
        }
        if total > limit {
            plan.no_room = Some(NoRoom::OwnLimit);
            return plan;
        }
    }
    if provider.is_some_and(|p| incoming > p.free + freed) {
        plan.no_room = Some(NoRoom::CloudFull);
    }
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
    fn the_cloud_auto_cleanup_is_like_the_local_one() {
        // 3 clips of 1 GB in the cloud, the oldest one a favorite
        let mut index = CloudIndex::default();
        let mut ids = Vec::new();
        for (i, favorite) in [(1, true), (2, false), (3, false)] {
            let id = ClipId::new_random();
            let created = Utc.timestamp_opt(1_790_000_000 + i, 0).unwrap();
            index.upsert(id, IndexEntry { size: GIB, created, favorite, ..Default::default() }, Utc::now());
            ids.push(id);
        }
        let roomy = Some(ProviderSpace { free: 100 * GIB, total: Some(200 * GIB) });
        // on, limit 3 GB, 1 GB coming: the oldest NON-favorite goes
        let plan = plan_cloud_limit(&index, true, 3, roomy, GIB);
        assert_eq!((plan.delete, plan.no_room), (vec![ids[1]], None));
        // favorites are never removed: a 1 GB limit cannot be met
        let plan = plan_cloud_limit(&index, true, 1, roomy, GIB);
        assert_eq!((plan.delete.len(), plan.no_room), (2, Some(NoRoom::OwnLimit)));
        // off (the default): the limit does nothing, nothing is removed
        assert_eq!(plan_cloud_limit(&index, false, 3, roomy, GIB), CloudLimitPlan::default());
    }

    #[test]
    fn a_full_cloud_never_makes_lag_delete_anything() {
        let mut index = CloudIndex::default();
        index.upsert(ClipId::new_random(), IndexEntry { size: GIB, ..Default::default() }, Utc::now());
        let full = Some(ProviderSpace { free: GIB / 2, total: Some(15 * GIB) });
        // the user's Drive is full of other files: an error, no cleanup - whether auto-cleanup is on or off
        for auto in [false, true] {
            let plan = plan_cloud_limit(&index, auto, 100, full, GIB);
            assert_eq!(plan, CloudLimitPlan { delete: vec![], no_room: Some(NoRoom::CloudFull) }, "auto-cleanup {auto}");
        }
        // what the user's own limit freed counts toward the cloud's space
        let mut two = index.clone();
        two.upsert(ClipId::new_random(), IndexEntry { size: GIB, ..Default::default() }, Utc::now());
        let plan = plan_cloud_limit(&two, true, 2, full, GIB);
        assert_eq!((plan.delete.len(), plan.no_room), (1, None));
        // unknown provider space: only our own rule
        assert_eq!(plan_cloud_limit(&index, false, 0, None, GIB), CloudLimitPlan::default());
    }
}
