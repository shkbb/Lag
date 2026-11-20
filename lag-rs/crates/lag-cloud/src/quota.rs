//! Storage limits with the cloud (spec §11). Plans only (pure); the library carries them out.
//!
//! Local library size limit (Settings > "Library size limit", auto-cleanup) keeps its meaning: the library
//! folder never grows past it, oldest videos first, screenshots never counted. With a cloud it first frees
//! space WITHOUT losing anything: a clip that is safely in a cloud only loses its local copy (it stays in the
//! library as "in the cloud"). Only if that is not enough are local-only clips deleted, as before. A clip
//! that is being uploaded or downloaded is never touched.
//!
//! Cloud limit (per account): when Lag's clips in a cloud pass the user's limit, either the oldest
//! non-favorite clips are removed from that cloud (if the user allowed it), or new uploads stop.
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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudLimitPlan {
    /// Clips to remove from this cloud (oldest non-favorites), when cleanup is allowed.
    pub delete: Vec<ClipId>,
    /// Still over the limit after that: uploads to this account stop ("cloud is full").
    pub full: bool,
}

/// Keeps Lag's clips in one account within `limit_bytes` (0 = no limit of our own), counting `incoming`
/// bytes about to be uploaded.
pub fn plan_cloud_limit(index: &CloudIndex, limit_bytes: u64, cleanup: bool, incoming: u64) -> CloudLimitPlan {
    if limit_bytes == 0 {
        return CloudLimitPlan::default();
    }
    let mut total = index.live_bytes() + incoming;
    if total <= limit_bytes {
        return CloudLimitPlan::default();
    }
    let mut plan = CloudLimitPlan::default();
    if cleanup {
        let mut oldest: Vec<_> = index.live().filter(|(_, e)| !e.favorite).collect();
        oldest.sort_by_key(|(_, e)| e.created);
        for (id, e) in oldest {
            if total <= limit_bytes {
                break;
            }
            total -= e.size;
            plan.delete.push(*id);
        }
    }
    plan.full = total > limit_bytes;
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
    fn the_cloud_limit_cleans_up_or_stops_uploads() {
        let mut index = CloudIndex::default();
        let mut ids = Vec::new();
        for (i, favorite) in [(1, true), (2, false), (3, false)] {
            let id = ClipId::new_random();
            let created = Utc.timestamp_opt(1_790_000_000 + i, 0).unwrap();
            index.upsert(id, IndexEntry { size: 10, created, favorite, ..Default::default() }, Utc::now());
            ids.push(id);
        }
        // 30 in the cloud + 10 coming, limit 25: the oldest NON-favorite clips go (the favorite stays)
        let plan = plan_cloud_limit(&index, 25, true, 10);
        assert_eq!(plan.delete, [ids[1], ids[2]]);
        assert!(!plan.full, "favorite + new clip = 20 fits");
        // favorites are never removed: if they alone are over the limit, uploads stop
        let plan = plan_cloud_limit(&index, 15, true, 10);
        assert_eq!((plan.delete.len(), plan.full), (2, true));
        // cleanup not allowed: nothing is removed, uploads stop
        assert_eq!(plan_cloud_limit(&index, 25, false, 10), CloudLimitPlan { delete: vec![], full: true });
        assert_eq!(plan_cloud_limit(&index, 0, true, 10), CloudLimitPlan::default(), "no limit of our own");
    }
}
