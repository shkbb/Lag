//! The cloud index `<root>/.lag/library.json` (spec §4): everything an account holds, so a new PC shows the
//! whole library before downloading a single video. Several PCs write it, so changes are MERGED entry by
//! entry (newer `updated` wins, deletes are tombstones), never written over each other.
use crate::ids::ClipId;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 1;
/// How long a delete is remembered: a PC offline for less than this cannot bring a deleted clip back.
pub const TOMBSTONE_DAYS: i64 = 90;

/// One clip in the cloud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexEntry {
    pub file_name: String,
    /// The provider's id of the video file (found again even if the user moved it).
    pub remote_id: Option<String>,
    /// Folder it was uploaded to (segments, root first): informational.
    pub folder: Vec<String>,
    pub game: Option<String>,
    pub exe: Option<String>,
    pub created: DateTime<Utc>,
    pub duration_seconds: Option<f64>,
    pub size: u64,
    pub sha256: Option<String>,
    pub favorite: bool,
    pub edited: bool,
    pub is_image: bool,
    /// The provider's id of `.lag/thumbs/<clip id>.jpg`.
    pub thumbnail_id: Option<String>,
    pub uploaded_at: Option<DateTime<Utc>>,
    /// Device name of the PC that uploaded it.
    pub uploaded_by: Option<String>,
    /// Last change of this entry (any field, or the delete).
    pub updated: DateTime<Utc>,
    /// Set = deleted from the cloud at this time (tombstone).
    pub deleted: Option<DateTime<Utc>>,
}

impl Default for IndexEntry {
    fn default() -> Self {
        Self {
            file_name: String::new(),
            remote_id: None,
            folder: Vec::new(),
            game: None,
            exe: None,
            created: DateTime::UNIX_EPOCH,
            duration_seconds: None,
            size: 0,
            sha256: None,
            favorite: false,
            edited: false,
            is_image: false,
            thumbnail_id: None,
            uploaded_at: None,
            uploaded_by: None,
            updated: DateTime::UNIX_EPOCH,
            deleted: None,
        }
    }
}

impl IndexEntry {
    pub fn is_live(&self) -> bool {
        self.deleted.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudIndex {
    pub schema: u32,
    pub entries: BTreeMap<ClipId, IndexEntry>,
}

impl Default for CloudIndex {
    fn default() -> Self {
        Self { schema: SCHEMA, entries: BTreeMap::new() }
    }
}

#[derive(Debug)]
pub enum IndexError {
    Json(serde_json::Error),
    /// Written by a newer Lag: never rewrite it (that would drop fields this version does not know).
    NewerSchema(u32),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "cloud index is not valid JSON: {e}"),
            Self::NewerSchema(v) => write!(f, "cloud index schema {v} is newer than this Lag ({SCHEMA})"),
        }
    }
}

impl std::error::Error for IndexError {}

impl CloudIndex {
    pub fn from_json(text: &str) -> Result<Self, IndexError> {
        let index: Self = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(IndexError::Json)?;
        if index.schema > SCHEMA {
            return Err(IndexError::NewerSchema(index.schema));
        }
        Ok(index)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("index serializes")
    }

    /// Clips that exist (not deleted).
    pub fn live(&self) -> impl Iterator<Item = (&ClipId, &IndexEntry)> {
        self.entries.iter().filter(|(_, e)| e.is_live())
    }

    /// Adds or replaces a clip, stamped `now`.
    pub fn upsert(&mut self, id: ClipId, mut entry: IndexEntry, now: DateTime<Utc>) {
        entry.updated = now;
        entry.deleted = None;
        self.entries.insert(id, entry);
    }

    /// Changes fields of a live clip (favorite, game, ...), stamped `now`. False if there is no such clip.
    pub fn edit(&mut self, id: ClipId, now: DateTime<Utc>, change: impl FnOnce(&mut IndexEntry)) -> bool {
        match self.entries.get_mut(&id).filter(|e| e.is_live()) {
            Some(e) => {
                change(e);
                e.updated = now;
                true
            }
            None => false,
        }
    }

    /// Marks a clip deleted (a tombstone other PCs learn from).
    pub fn delete(&mut self, id: ClipId, now: DateTime<Utc>) {
        let e = self.entries.entry(id).or_default();
        e.deleted = Some(now);
        e.updated = now;
    }

    /// Takes `other`'s changes: per clip the newer `updated` wins; on an exact tie a delete wins (the safer
    /// outcome is to not show a clip that may be gone). Any order of merging gives the same result.
    pub fn merge(&mut self, other: &CloudIndex) {
        for (id, theirs) in &other.entries {
            match self.entries.get(id) {
                Some(ours) if !newer(theirs, ours) => {}
                _ => {
                    self.entries.insert(*id, theirs.clone());
                }
            }
        }
    }

    /// Forgets tombstones older than [`TOMBSTONE_DAYS`].
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let limit = now - Duration::days(TOMBSTONE_DAYS);
        self.entries.retain(|_, e| e.deleted.is_none_or(|d| d > limit));
    }

    /// Bytes of the clips that exist (for the cloud usage bar and the cloud limit).
    pub fn live_bytes(&self) -> u64 {
        self.live().map(|(_, e)| e.size).sum()
    }
}

fn newer(a: &IndexEntry, b: &IndexEntry) -> bool {
    a.updated > b.updated || (a.updated == b.updated && a.deleted.is_some() && b.deleted.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_790_000_000 + secs, 0).unwrap()
    }
    fn clip(name: &str) -> IndexEntry {
        IndexEntry { file_name: name.into(), size: 100, ..Default::default() }
    }

    #[test]
    fn two_pcs_merge_without_losing_changes() {
        let (a_id, b_id) = (ClipId::new_random(), ClipId::new_random());
        let mut pc_a = CloudIndex::default();
        pc_a.upsert(a_id, clip("a.mp4"), t(0));
        let mut pc_b = pc_a.clone();
        // A favorites its clip, B uploads a new one, both offline from each other
        pc_a.edit(a_id, t(10), |e| e.favorite = true);
        pc_b.upsert(b_id, clip("b.mp4"), t(5));
        let mut ab = pc_a.clone();
        ab.merge(&pc_b);
        let mut ba = pc_b.clone();
        ba.merge(&pc_a);
        assert_eq!(ab, ba, "merge order does not matter");
        assert!(ab.entries[&a_id].favorite);
        assert_eq!(ab.live().count(), 2);
    }

    #[test]
    fn deletes_are_tombstones_until_they_expire() {
        let id = ClipId::new_random();
        let mut cloud = CloudIndex::default();
        cloud.upsert(id, clip("a.mp4"), t(0));
        let offline_pc = cloud.clone();
        cloud.delete(id, t(100));
        // the offline PC comes back with its old copy: the clip stays deleted
        cloud.merge(&offline_pc);
        assert_eq!(cloud.live().count(), 0);
        // re-uploading later brings it back
        let mut later = CloudIndex::default();
        later.upsert(id, clip("a.mp4"), t(200));
        cloud.merge(&later);
        assert_eq!(cloud.live().count(), 1);
        // an edit of a deleted clip does nothing
        cloud.delete(id, t(300));
        assert!(!cloud.edit(id, t(400), |e| e.favorite = true));
        cloud.prune(t(300) + Duration::days(TOMBSTONE_DAYS - 1));
        assert_eq!(cloud.entries.len(), 1, "still remembered");
        cloud.prune(t(300) + Duration::days(TOMBSTONE_DAYS + 1));
        assert!(cloud.entries.is_empty(), "forgotten");
    }

    #[test]
    fn tie_goes_to_the_delete() {
        let id = ClipId::new_random();
        let mut a = CloudIndex::default();
        a.upsert(id, clip("a.mp4"), t(5));
        let mut b = a.clone();
        b.delete(id, t(5));
        a.merge(&b);
        assert_eq!(a.live().count(), 0);
    }

    #[test]
    fn json_round_trip_and_newer_schema_is_refused() {
        let mut index = CloudIndex::default();
        index.upsert(ClipId::new_random(), IndexEntry { game: Some("CS2".into()), ..clip("a.mp4") }, t(1));
        assert_eq!(CloudIndex::from_json(&index.to_json()).unwrap(), index);
        assert_eq!(index.live_bytes(), 100);
        let future = index.to_json().replacen("\"schema\": 1", "\"schema\": 99", 1);
        assert!(matches!(CloudIndex::from_json(&future), Err(IndexError::NewerSchema(99))));
        assert!(matches!(CloudIndex::from_json("{oops"), Err(IndexError::Json(_))));
    }
}
