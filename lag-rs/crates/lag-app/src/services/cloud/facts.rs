//! What the cloud worker reads about a clip on this PC (file, metadata sidecar, cached thumbnail) and what it
//! writes back after a download. The same files the library uses: nothing here invents a second store.
use chrono::{DateTime, Utc};
use lag_cloud::index::IndexEntry;
use lag_cloud::ClipId;
use lag_core::metadata::{ClipMetadata, ClipMetadataStore};
use std::path::{Path, PathBuf};

pub(super) fn is_image(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["png", "jpg", "jpeg"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The library's thumbnail cache file for a clip path or cloud key (views/library/clip.rs uses the same rule).
pub(crate) fn thumbnail_path(path_or_key: &str) -> Option<PathBuf> {
    lag_core::hardware::local_appdata()
        .map(|p| p.join("Lag").join("thumbnails").join(format!("{}.jpg", lag_core::metadata::path_hash(path_or_key))))
}

/// The clip's cloud id, given one now (and stored in its metadata) if it never had one.
pub(super) fn ensure_id(store: &ClipMetadataStore, path: &str) -> ClipId {
    if let Some(id) = store.read(path).and_then(|m| m.cloud_id).and_then(|s| ClipId::parse(&s)) {
        return id;
    }
    let id = ClipId::new_random();
    store.set_cloud_id(path, &id.to_string());
    id
}

pub(super) fn known_id(store: &ClipMetadataStore, path: &str) -> Option<ClipId> {
    store.read(path).and_then(|m| m.cloud_id).and_then(|s| ClipId::parse(&s))
}

/// Everything the cloud index records about a local clip, plus its thumbnail (JPEG) if the library made one.
/// None if the file is gone.
pub(super) fn read(store: Option<&ClipMetadataStore>, path: &str) -> Option<(IndexEntry, Option<Vec<u8>>)> {
    let fs = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let created: DateTime<Utc> = fs.created().or_else(|_| fs.modified()).map(Into::into).unwrap_or_else(|_| Utc::now());
    let meta = store.and_then(|s| s.read(path)).unwrap_or_default();
    let entry = IndexEntry {
        file_name: Path::new(path).file_name()?.to_string_lossy().into_owned(),
        game: meta.game.filter(|g| !g.trim().is_empty()),
        exe: meta.exe,
        created,
        duration_seconds: meta.duration_seconds,
        size: fs.len(),
        favorite: meta.favorite,
        edited: meta.edited,
        is_image: is_image(path),
        ..Default::default()
    };
    let thumbnail = thumbnail_path(path).and_then(|t| std::fs::read(t).ok());
    Some((entry, thumbnail))
}

/// `dir/name`, or `dir/name (2).ext`... if that is taken.
pub(super) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').map_or((name, String::new()), |(s, e)| (s, format!(".{e}")));
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("a free name")
}

/// A clip came down from the cloud: it gets its metadata (same id, game, favorite...) and its original date,
/// so the library shows it once, where it belongs.
pub(super) fn after_download(store: Option<&ClipMetadataStore>, path: &Path, id: ClipId, entry: &IndexEntry) {
    if let Some(store) = store {
        store.write(
            &path.to_string_lossy(),
            &ClipMetadata {
                game: entry.game.clone(),
                exe: entry.exe.clone(),
                favorite: entry.favorite,
                edited: entry.edited,
                duration_seconds: entry.duration_seconds,
                cloud_id: Some(id.to_string()),
            },
        );
    }
    use std::os::windows::fs::FileTimesExt;
    let when: std::time::SystemTime = entry.created.into();
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_times(std::fs::FileTimes::new().set_modified(when).set_created(when));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_facts_ids_and_downloads() {
        let dir = std::env::temp_dir().join(format!("lag-cloud-facts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ClipMetadataStore::new(dir.join("meta"));
        let clip = dir.join("a.mp4");
        std::fs::write(&clip, b"12345").unwrap();
        let path = clip.to_string_lossy().into_owned();
        store.update(&path, |m| {
            m.game = Some("CS2".into());
            m.favorite = true;
        });
        let (entry, _) = read(Some(&store), &path).unwrap();
        assert_eq!((entry.size, entry.game.as_deref(), entry.favorite, entry.is_image), (5, Some("CS2"), true, false));
        assert!(read(Some(&store), &dir.join("gone.mp4").to_string_lossy()).is_none());

        let id = ensure_id(&store, &path);
        assert_eq!(ensure_id(&store, &path), id, "an id is given once");
        assert_eq!(known_id(&store, &path), Some(id));

        assert_eq!(unique_path(&dir, "a.mp4"), dir.join("a (2).mp4"));
        let down = unique_path(&dir, "b.mp4");
        std::fs::write(&down, b"x").unwrap();
        let created = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 1, 2, 3, 4, 5).unwrap();
        after_download(Some(&store), &down, id, &IndexEntry { created, game: Some("GTA V".into()), ..Default::default() });
        let meta = store.read(&down.to_string_lossy()).unwrap();
        assert_eq!((meta.game.as_deref(), meta.cloud_id), (Some("GTA V"), Some(id.to_string())));
        let when: DateTime<Utc> = std::fs::metadata(&down).unwrap().created().unwrap().into();
        assert_eq!(when, created, "the clip keeps its original date");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
