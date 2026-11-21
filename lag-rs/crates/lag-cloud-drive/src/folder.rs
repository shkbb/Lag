//! A folder as a cloud: a NAS share (`\\nas\share`), a mapped network drive, or any folder another tool syncs
//! (Syncthing, Synology Drive, Nextcloud desktop...). Files are files: a file's id is its full path, a small
//! file's revision is its modified time + size, Lag's file properties are notes in `.lag/props` (sidecar.rs).
//! Two PCs writing the shared index at once take turns through a lock file next to it.
use crate::sidecar;
use lag_cloud::provider::*;
use lag_cloud::queue::UploadSession;
use lag_cloud::quota::ProviderSpace;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const CHUNK: usize = 4 << 20;
/// A lock older than this was left by a PC that went away mid-write.
const STALE_LOCK: Duration = Duration::from_secs(30);

pub struct FolderStorage {
    base: PathBuf,
}

fn io(e: std::io::Error) -> CloudError {
    match e.kind() {
        std::io::ErrorKind::NotFound => CloudError::NotFound,
        std::io::ErrorKind::StorageFull => CloudError::Full,
        // a NAS that went to sleep, a dropped network share: try again later
        _ => CloudError::Transient(e.to_string()),
    }
}

fn revision(meta: &std::fs::Metadata) -> Revision {
    let modified = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).unwrap_or_default();
    Revision(format!("{}-{}", modified.as_nanos(), meta.len()))
}

/// Holds `<dir>/index.lock` while alive.
struct Lock(PathBuf);

impl Lock {
    fn take(dir: &Path) -> CloudResult<Self> {
        let path = dir.join("index.lock");
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE_LOCK);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() > Duration::from_secs(10) {
                        return Err(CloudError::Transient("the shared library is busy".into()));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(io(e)),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl FolderStorage {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    fn at(&self, path: &[String]) -> PathBuf {
        path.iter().fold(self.base.clone(), |p, s| p.join(s))
    }

    /// '/'-separated path below the base (for the notes).
    fn relative(&self, file: &Path) -> String {
        file.strip_prefix(&self.base).unwrap_or(file).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
    }

    /// `<base>/<root>/.lag/props` for a file somewhere under `<base>/<root>/`.
    fn notes_dir(&self, file: &Path) -> PathBuf {
        let rel = file.strip_prefix(&self.base).unwrap_or(file);
        let root = rel.components().next().map(|c| self.base.join(c.as_os_str())).unwrap_or_else(|| self.base.clone());
        sidecar::DIR.iter().fold(root, |p, s| p.join(s))
    }

    fn notes(&self, dir: &Path) -> Vec<sidecar::Note> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
            .collect()
    }
}

impl CloudProvider for FolderStorage {
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String> {
        let dir = self.at(path);
        std::fs::create_dir_all(&dir).map_err(io)?;
        Ok(dir.to_string_lossy().into_owned())
    }

    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let file = self.at(path);
        match std::fs::read(&file) {
            Ok(bytes) => Ok(Some((bytes, revision(&std::fs::metadata(&file).map_err(io)?)))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome> {
        let file = self.at(path);
        let dir = file.parent().ok_or_else(|| CloudError::Permanent("no folder".into()))?;
        std::fs::create_dir_all(dir).map_err(io)?;
        let _lock = Lock::take(dir)?;
        let current = std::fs::metadata(&file).ok().map(|m| revision(&m));
        if current.as_ref() != expected {
            return Ok(WriteOutcome::Conflict);
        }
        let tmp = file.with_extension("lagtmp");
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, &file).map_err(io)?;
        Ok(WriteOutcome::Written(revision(&std::fs::metadata(&file).map_err(io)?)))
    }

    fn upload(
        &self,
        folder_id: &str,
        name: &str,
        size: u64,
        source: &mut dyn ReadSeek,
        props: &FileProps,
        session: Option<&UploadSession>,
        on_session: &mut dyn FnMut(UploadSession),
        progress: Progress,
    ) -> CloudResult<RemoteFile> {
        let folder = PathBuf::from(folder_id);
        let target = folder.join(name);
        let part = folder.join(format!("{name}.lagpart"));
        // resume a copy that stopped: whatever reached the share stays
        let resumed = session.is_some_and(|s| s.url == part.to_string_lossy());
        let mut offset = if resumed { std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0).min(size) } else { 0 };
        let mut out = std::fs::OpenOptions::new().create(true).write(true).truncate(!resumed).open(&part).map_err(io)?;
        out.set_len(offset).map_err(io)?;
        out.seek(SeekFrom::Start(offset)).map_err(io)?;
        source.seek(SeekFrom::Start(offset)).map_err(|e| CloudError::Permanent(e.to_string()))?;
        let url = part.to_string_lossy().into_owned();
        on_session(UploadSession { url: url.clone(), offset });
        let mut buf = vec![0u8; CHUNK];
        while offset < size {
            let n = CHUNK.min((size - offset) as usize);
            source.read_exact(&mut buf[..n]).map_err(|e| CloudError::Permanent(e.to_string()))?;
            out.write_all(&buf[..n]).map_err(io)?;
            offset += n as u64;
            on_session(UploadSession { url: url.clone(), offset });
            if !progress(offset) {
                return Err(CloudError::Cancelled);
            }
        }
        out.flush().map_err(io)?;
        drop(out);
        if target.exists() {
            std::fs::remove_file(&target).map_err(io)?;
        }
        std::fs::rename(&part, &target).map_err(io)?;

        if sidecar::tagged(props) {
            let dir = self.notes_dir(&target);
            std::fs::create_dir_all(&dir).map_err(io)?;
            let rel = self.relative(&target);
            let note = sidecar::Note { path: rel.clone(), size, props: props.clone() };
            std::fs::write(dir.join(sidecar::name(&rel)), serde_json::to_vec_pretty(&note).unwrap_or_default()).map_err(io)?;
        }
        Ok(RemoteFile {
            id: target.to_string_lossy().into_owned(),
            name: name.to_owned(),
            size,
            hash: None,
            props: props.clone(),
        })
    }

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()> {
        let mut file = std::fs::File::open(file_id).map_err(io)?;
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            let n = file.read(&mut buf).map_err(io)?;
            if n == 0 {
                return Ok(());
            }
            sink.write_all(&buf[..n]).map_err(|e| CloudError::Permanent(e.to_string()))?;
            done += n as u64;
            if !progress(done) {
                return Err(CloudError::Cancelled);
            }
        }
    }

    fn delete(&self, file_id: &str) -> CloudResult<()> {
        let file = PathBuf::from(file_id);
        std::fs::remove_file(&file).map_err(io)?;
        let _ = std::fs::remove_file(self.notes_dir(&file).join(sidecar::name(&self.relative(&file))));
        Ok(())
    }

    fn list_tagged(&self, root_id: &str) -> CloudResult<Vec<RemoteFile>> {
        let dir = sidecar::DIR.iter().fold(PathBuf::from(root_id), |p, s| p.join(s));
        Ok(self
            .notes(&dir)
            .into_iter()
            .filter_map(|note| {
                let file = note.path.split('/').fold(self.base.clone(), |p, s| p.join(s));
                let meta = std::fs::metadata(&file).ok()?;
                Some(RemoteFile {
                    id: file.to_string_lossy().into_owned(),
                    name: file.file_name()?.to_string_lossy().into_owned(),
                    size: meta.len(),
                    hash: None,
                    props: note.props,
                })
            })
            .collect())
    }

    fn space(&self) -> CloudResult<ProviderSpace> {
        crate::win::disk_space(&self.base).ok_or_else(|| CloudError::Transient("the folder cannot be reached".into()))
    }

    fn stream(&self, file_id: &str) -> CloudResult<StreamSource> {
        // a path the player opens directly (UNC paths too)
        Ok(StreamSource { url: file_id.to_owned(), headers: Vec::new() })
    }

    fn web_link(&self, _file_id: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lag_cloud::index::IndexEntry;
    use lag_cloud::layout::SortMode;
    use lag_cloud::sync::{AccountClient, UploadRequest};
    use lag_cloud::ClipId;
    use std::io::Cursor;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lag-folder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_nas_folder_works_like_any_cloud() {
        let base = temp("e2e");
        let nas = FolderStorage::new(&base);
        let pc = AccountClient { provider: &nas, root: "Lag".into(), sort: SortMode::ByGame, device: "A".into() };
        let id = ClipId::new_random();
        let data = vec![7u8; 10_000];
        let entry = IndexEntry { file_name: "cs.mp4".into(), game: Some("Counter-Strike 2".into()), size: 10_000, ..Default::default() };
        let req = UploadRequest { id, entry, source: &mut Cursor::new(&data), thumbnail_jpeg: Some(b"jpg".to_vec()) };
        let stored = pc.upload_clip(req, None, &mut |_| {}, &mut |_| true).unwrap();
        // the tree people see on their NAS
        assert!(base.join("Lag").join("Counter-Strike 2").join("cs.mp4").exists());
        assert!(base.join("Lag").join(".lag").join("library.json").exists());
        // index, download, space, stream
        let (index, _) = pc.load_index().unwrap();
        assert_eq!(index.entries[&id].size, 10_000);
        let mut back = Vec::new();
        nas.download(stored.remote_id.as_deref().unwrap(), &mut back, &mut |_| true).unwrap();
        assert_eq!(back, data);
        assert!(nas.space().unwrap().total.is_some());
        // lost index: rebuilt from the notes
        let rebuilt = pc.rescan().unwrap();
        assert_eq!(rebuilt.entries[&id].game.as_deref(), Some("Counter-Strike 2"));
        assert!(rebuilt.entries[&id].thumbnail_id.is_some());
        // delete: file and note go
        pc.delete_clip(id).unwrap();
        assert!(!base.join("Lag").join("Counter-Strike 2").join("cs.mp4").exists());
        assert!(pc.rescan().unwrap().entries.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_stopped_copy_resumes_and_the_index_is_guarded() {
        let base = temp("resume");
        let nas = FolderStorage::new(&base);
        let folder = nas.ensure_folder(&["Lag".into(), "Desktop".into()]).unwrap();
        let data: Vec<u8> = (0..(CHUNK * 2 + 10)).map(|i| i as u8).collect();
        let mut session = None;
        let err = nas
            .upload(&folder, "a.mp4", data.len() as u64, &mut Cursor::new(&data), &FileProps::new(), None, &mut |s| session = Some(s), &mut |done| done < CHUNK as u64)
            .unwrap_err();
        assert_eq!(err, CloudError::Cancelled);
        let session = session.unwrap();
        assert_eq!(session.offset, CHUNK as u64);
        let file = nas
            .upload(&folder, "a.mp4", data.len() as u64, &mut Cursor::new(&data), &FileProps::new(), Some(&session), &mut |_| {}, &mut |_| true)
            .unwrap();
        assert_eq!(std::fs::read(&file.id).unwrap(), data);
        assert!(!Path::new(&format!("{}.lagpart", file.id)).exists());

        // the index: written only if nobody changed it in between
        let path: Vec<String> = ["Lag", ".lag", "library.json"].iter().map(|s| s.to_string()).collect();
        let WriteOutcome::Written(rev) = nas.write_small(&path, b"1", None).unwrap() else { panic!() };
        assert_eq!(nas.write_small(&path, b"2", None).unwrap(), WriteOutcome::Conflict);
        std::thread::sleep(Duration::from_millis(20));
        assert!(matches!(nas.write_small(&path, b"22", Some(&rev)).unwrap(), WriteOutcome::Written(_)));
        assert_eq!(nas.read_small(&path).unwrap().unwrap().0, b"22");
        assert_eq!(nas.write_small(&path, b"3", Some(&rev)).unwrap(), WriteOutcome::Conflict, "stale revision");
        let _ = std::fs::remove_dir_all(&base);
    }
}
