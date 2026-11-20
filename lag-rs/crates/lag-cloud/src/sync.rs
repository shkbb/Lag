//! Talking to one account (spec §2-§4, §8, §11): read the index, change it safely while other PCs do the same,
//! upload a clip with its thumbnail into the game's folder, delete, sync favorites, run the cloud's
//! auto-cleanup, and rebuild a lost index from the files themselves. Blocking; runs on the cloud worker.
use crate::ids::ClipId;
use crate::index::{CloudIndex, IndexEntry, IndexError};
use crate::layout::{self, SortMode};
use crate::provider::*;
use crate::queue::UploadSession;
use crate::quota::{plan_cloud_limit, CloudLimitPlan};
use crate::settings::WhenFull;
use chrono::{DateTime, Utc};
use std::io::Read;

/// How often a read-merge-write of the index is retried when other PCs keep writing in between.
const COMMIT_TRIES: usize = 8;

/// File properties beyond the clip id, so a rescan can rebuild the index without it (spec §4).
mod meta {
    pub const GAME: &str = "lagGame";
    pub const CREATED: &str = "lagCreated";
    pub const DURATION: &str = "lagDuration";
    pub const IMAGE: &str = "lagImage";
}

pub struct AccountClient<'a> {
    pub provider: &'a dyn CloudProvider,
    /// Lag's root folder name in this cloud (settings).
    pub root: String,
    pub sort: SortMode,
    /// Shown as "uploaded by" on other PCs.
    pub device: String,
}

/// A clip to upload: its facts (written into the index and onto the file) and its bytes.
pub struct UploadRequest<'r> {
    pub id: ClipId,
    pub entry: IndexEntry,
    pub source: &'r mut dyn Read,
    pub thumbnail_jpeg: Option<Vec<u8>>,
}

impl AccountClient<'_> {
    /// The account's index; an empty one if the account has none yet. A corrupt index is rebuilt from the
    /// files (never just overwritten); one from a newer Lag is refused (it may hold fields we would drop).
    pub fn load_index(&self) -> CloudResult<(CloudIndex, Option<Revision>)> {
        let path = layout::index_path(&self.root);
        match self.provider.read_small(&path)? {
            None => Ok((CloudIndex::default(), None)),
            Some((bytes, rev)) => match CloudIndex::from_json(&String::from_utf8_lossy(&bytes)) {
                Ok(index) => Ok((index, Some(rev))),
                Err(IndexError::NewerSchema(v)) => {
                    Err(CloudError::Permanent(format!("the cloud library was written by a newer Lag (schema {v})")))
                }
                Err(IndexError::Json(e)) => {
                    log::warn!("[Cloud] index is corrupt ({e}), rebuilding it from the files");
                    Ok((self.rescan()?, Some(rev)))
                }
            },
        }
    }

    /// Changes the index safely while other PCs may write it too: read the latest, apply `change` to it,
    /// write only if nobody wrote in between, otherwise start over from their version. `change` therefore
    /// must be repeatable (set a field, insert an entry): it is applied to whatever is current.
    pub fn commit(&self, change: &dyn Fn(&mut CloudIndex)) -> CloudResult<CloudIndex> {
        let path = layout::index_path(&self.root);
        self.provider.ensure_folder(&path[..path.len() - 1])?;
        for _ in 0..COMMIT_TRIES {
            let (mut index, rev) = self.load_index()?;
            change(&mut index);
            index.prune(Utc::now());
            match self.provider.write_small(&path, index.to_json().as_bytes(), rev.as_ref())? {
                WriteOutcome::Written(_) => return Ok(index),
                WriteOutcome::Conflict => continue,
            }
        }
        Err(CloudError::Transient("the cloud library kept changing; will retry".into()))
    }

    /// Uploads a clip (resuming `session` if given) and its thumbnail into `<root>/<game>/...`, then adds it
    /// to the index. Returns the entry as stored.
    pub fn upload_clip(
        &self,
        req: UploadRequest,
        session: Option<&UploadSession>,
        on_session: &mut dyn FnMut(UploadSession),
        progress: Progress,
    ) -> CloudResult<IndexEntry> {
        let UploadRequest { id, mut entry, source, thumbnail_jpeg } = req;
        let created_local = entry.created.naive_utc();
        let folder = layout::clip_folder(&self.root, self.sort, entry.game.as_deref(), created_local);
        let folder_id = self.provider.ensure_folder(&folder)?;
        let mut file_props = FileProps::new();
        file_props.insert(props::CLIP_ID.into(), id.to_string());
        file_props.insert(props::KIND.into(), props::KIND_CLIP.into());
        file_props.insert(meta::CREATED.into(), entry.created.to_rfc3339());
        if let Some(game) = &entry.game {
            file_props.insert(meta::GAME.into(), game.clone());
        }
        if let Some(d) = entry.duration_seconds {
            file_props.insert(meta::DURATION.into(), d.to_string());
        }
        if entry.is_image {
            file_props.insert(meta::IMAGE.into(), "1".into());
        }
        let name = layout::safe_name(&entry.file_name, "clip.mp4");
        let file =
            self.provider.upload(&folder_id, &name, entry.size, source, &file_props, session, on_session, progress)?;

        if let Some(jpeg) = thumbnail_jpeg {
            let thumb_path = layout::thumbnail_path(&self.root, id);
            let thumb_folder = self.provider.ensure_folder(&thumb_path[..thumb_path.len() - 1])?;
            let mut thumb_props = FileProps::new();
            thumb_props.insert(props::KIND.into(), props::KIND_THUMB.into());
            thumb_props.insert(props::CLIP_ID.into(), id.to_string());
            let thumb_name = thumb_path.last().cloned().unwrap_or_default();
            // a thumbnail is a nicety: the clip is uploaded even if it fails
            match self.provider.upload(
                &thumb_folder,
                &thumb_name,
                jpeg.len() as u64,
                &mut jpeg.as_slice(),
                &thumb_props,
                None,
                &mut |_| {},
                &mut |_| true,
            ) {
                Ok(t) => entry.thumbnail_id = Some(t.id),
                Err(e) => log::warn!("[Cloud] thumbnail of {id} not uploaded: {e}"),
            }
        }

        entry.file_name = name;
        entry.remote_id = Some(file.id);
        entry.folder = folder;
        entry.sha256 = file.hash.or(entry.sha256);
        entry.uploaded_at = Some(Utc::now());
        entry.uploaded_by = Some(self.device.clone());
        let stored = entry.clone();
        self.commit(&|index| index.upsert(id, stored.clone(), Utc::now()))?;
        Ok(entry)
    }

    /// Removes a clip from this cloud: video, thumbnail, and a tombstone so other PCs drop it too.
    pub fn delete_clip(&self, id: ClipId) -> CloudResult<()> {
        let (index, _) = self.load_index()?;
        if let Some(entry) = index.entries.get(&id).filter(|e| e.is_live()) {
            for file in [&entry.remote_id, &entry.thumbnail_id].into_iter().flatten() {
                match self.provider.delete(file) {
                    Ok(()) | Err(CloudError::NotFound) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.commit(&|index| index.delete(id, Utc::now()))?;
        Ok(())
    }

    /// Favorite, game, edited... changed on this PC: other PCs see it.
    pub fn sync_meta(&self, id: ClipId, change: &dyn Fn(&mut IndexEntry)) -> CloudResult<()> {
        self.commit(&|index| {
            index.edit(id, Utc::now(), change);
        })?;
        Ok(())
    }

    /// The cloud's auto-cleanup before uploading `incoming` bytes: removes what the plan says and reports
    /// whether the clip fits now.
    pub fn make_room(&self, max_size_gb: u32, when_full: WhenFull, incoming: u64) -> CloudResult<CloudLimitPlan> {
        let (index, _) = self.load_index()?;
        let space = self.provider.space()?;
        let plan = plan_cloud_limit(&index, max_size_gb, Some(space), when_full, incoming);
        for id in &plan.delete {
            log::info!("[Cloud] auto-cleanup: removing {id} from the cloud");
            self.delete_clip(*id)?;
        }
        Ok(plan)
    }

    /// A clip that is only in the cloud plays from there; "Save to this PC" downloads it.
    pub fn stream(&self, entry: &IndexEntry) -> CloudResult<StreamSource> {
        self.provider.stream(entry.remote_id.as_deref().ok_or(CloudError::NotFound)?)
    }

    /// Rebuilds the index from Lag's files in the cloud (they carry their clip id and facts), wherever the
    /// user moved them. Favorites are not on the files; they come back as not favorite.
    pub fn rescan(&self) -> CloudResult<CloudIndex> {
        let root = self.provider.ensure_folder(&[layout::safe_name(&self.root, layout::DEFAULT_ROOT)])?;
        let files = self.provider.list_tagged(&root)?;
        let mut index = CloudIndex::default();
        let now = Utc::now();
        for f in files.iter().filter(|f| f.props.get(props::KIND).map(String::as_str) == Some(props::KIND_CLIP)) {
            let Some(id) = f.props.get(props::CLIP_ID).and_then(|s| ClipId::parse(s)) else { continue };
            let created = f
                .props
                .get(meta::CREATED)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map_or(now, |d| d.with_timezone(&Utc));
            let entry = IndexEntry {
                file_name: f.name.clone(),
                remote_id: Some(f.id.clone()),
                game: f.props.get(meta::GAME).cloned(),
                created,
                duration_seconds: f.props.get(meta::DURATION).and_then(|s| s.parse().ok()),
                size: f.size,
                sha256: f.hash.clone(),
                is_image: f.props.contains_key(meta::IMAGE),
                ..Default::default()
            };
            index.upsert(id, entry, now);
        }
        let thumbs = files.iter().filter(|f| f.props.get(props::KIND).map(String::as_str) == Some(props::KIND_THUMB));
        for t in thumbs {
            if let Some(id) = t.props.get(props::CLIP_ID).and_then(|s| ClipId::parse(s)) {
                if let Some(e) = index.entries.get_mut(&id) {
                    e.thumbnail_id = Some(t.id.clone());
                }
            }
        }
        Ok(index)
    }
}
