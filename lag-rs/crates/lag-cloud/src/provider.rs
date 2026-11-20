//! What every cloud must do (Google Drive, Dropbox, OneDrive, and the in-memory cloud of the tests). Blocking
//! calls: they run on the cloud worker thread, never on the UI thread. Paths are folder segments, root first
//! (layout.rs); files are found again by id, never by path.
use crate::queue::UploadSession;
use crate::quota::ProviderSpace;
use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};

/// A file being uploaded: read from the start, or from wherever the cloud says a resumed upload stands.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Provider-side metadata Lag stores on each file (Drive `appProperties`), so a rescan of the cloud finds
/// Lag's clips even after the user moved or renamed them, and can rebuild a lost index.
pub type FileProps = BTreeMap<String, String>;

/// Property names Lag writes on its files.
pub mod props {
    pub const CLIP_ID: &str = "lagClipId";
    pub const KIND: &str = "lagKind";
    pub const KIND_CLIP: &str = "clip";
    pub const KIND_THUMB: &str = "thumb";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub id: String,
    pub name: String,
    pub size: u64,
    /// Content hash if the provider reports one (Drive sha256Checksum, Dropbox content_hash).
    pub hash: Option<String>,
    pub props: FileProps,
}

/// Version of a small file (the index): written back only if nobody changed it in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Written(Revision),
    /// Someone else wrote it since `expected`: read again, merge, retry.
    Conflict,
}

/// How to play a clip straight from the cloud (owner, 2026-11-20: cloud clips play from the cloud; saving
/// them locally is a separate action). mpv opens the URL with these headers (range requests: seeking works).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSource {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudError {
    /// Network trouble, timeouts, server busy (5xx, 429): retry later.
    Transient(String),
    /// Sign-in expired or revoked: the account's jobs wait for the user.
    SignIn,
    /// No room left in the cloud itself (the upload fails; Lag deletes nothing for it).
    Full,
    /// Over the user's own limit for Lag in this cloud and auto-cleanup is off or cannot free enough.
    OverLimit,
    NotFound,
    /// The transfer was cancelled by Lag (clip deleted meanwhile, app closing).
    Cancelled,
    /// Anything retrying cannot fix.
    Permanent(String),
}

impl CloudError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

impl std::fmt::Display for CloudError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(e) => write!(f, "temporary cloud error: {e}"),
            Self::SignIn => f.write_str("sign-in needed"),
            Self::Full => f.write_str("not enough space in the cloud"),
            Self::OverLimit => f.write_str("over the cloud limit set in Lag"),
            Self::NotFound => f.write_str("not found in the cloud"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Permanent(e) => write!(f, "cloud error: {e}"),
        }
    }
}

impl std::error::Error for CloudError {}

pub type CloudResult<T> = Result<T, CloudError>;

/// Called with bytes done; returns false to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(u64) -> bool;

pub trait CloudProvider: Send + Sync {
    /// The folder at `path` (created with its parents when missing); returns its id.
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String>;

    /// A small file by path (the index): its bytes and revision, None if it does not exist.
    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>>;

    /// Writes a small file by path, only if its revision is still `expected` (None = it must not exist yet).
    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome>;

    /// Uploads `size` bytes from `source` as `name` into folder `folder_id`. `session` resumes an earlier
    /// attempt: the provider asks the cloud how much it really has and seeks `source` there itself (it may
    /// differ from the stored offset when the connection dropped mid-chunk). `on_session` is told every new
    /// session/offset so the queue can persist it.
    #[allow(clippy::too_many_arguments)]
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
    ) -> CloudResult<RemoteFile>;

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()>;

    fn delete(&self, file_id: &str) -> CloudResult<()>;

    /// Every file under the folder `root_id` carrying Lag's properties (rescan: rebuild a lost index).
    fn list_tagged(&self, root_id: &str) -> CloudResult<Vec<RemoteFile>>;

    fn space(&self) -> CloudResult<ProviderSpace>;

    /// Play from the cloud without downloading.
    fn stream(&self, file_id: &str) -> CloudResult<StreamSource>;

    /// Link to open the file in the provider's web UI ("Open in Google Drive").
    fn web_link(&self, file_id: &str) -> Option<String>;
}
