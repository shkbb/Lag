//! File properties for storages that have none of their own (a NAS folder, WebDAV): Google Drive keeps Lag's
//! `appProperties` on the file itself; here each tagged file gets a small JSON note in
//! `<root>/.lag/props/<hash of its path>.json` with its path and properties, so a rescan can still rebuild the
//! cloud index from the files alone (docs/spec/cloud.md §4). Named by path: deleting a file knows its note
//! without listing them (a file moved by hand is not found by its path anyway).
use lag_cloud::provider::{props, FileProps};
use serde::{Deserialize, Serialize};

/// Folder (under Lag's root) the notes live in.
pub const DIR: [&str; 2] = [".lag", "props"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// The file's path relative to the storage base, '/'-separated.
    pub path: String,
    pub size: u64,
    pub props: FileProps,
}

/// Whether a file needs a note (it carries a clip id).
pub fn tagged(file_props: &FileProps) -> bool {
    file_props.contains_key(props::CLIP_ID)
}

/// The note's file name for the file at `relative_path` ('/'-separated, case kept).
pub fn name(relative_path: &str) -> String {
    let hash = crate::win::sha256(relative_path.as_bytes());
    let hex: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{hex}.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notes_are_named_by_path_and_only_for_tagged_files() {
        let mut p = FileProps::new();
        assert!(!tagged(&p));
        p.insert(props::CLIP_ID.into(), "x".into());
        assert!(tagged(&p));
        let a = name("Lag/Counter-Strike 2/a.mp4");
        assert_eq!(a.len(), 32 + 5);
        assert_eq!(a, name("Lag/Counter-Strike 2/a.mp4"));
        assert_ne!(a, name("Lag/Counter-Strike 2/b.mp4"));
        assert!(!name("../../evil").contains('/'), "never a path");
    }
}
