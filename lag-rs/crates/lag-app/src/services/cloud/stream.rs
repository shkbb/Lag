//! Clips that live only in the cloud, as the rest of the app sees them: a path-like key
//! `cloud:<clip id>/<file name>` (the extension still tells a video from a screenshot, the name gives the
//! title), and how the player turns that key into something it can play.
use super::providers;
use lag_cloud::provider::StreamSource;
use lag_cloud::settings::AccountSettings;
use lag_cloud::ClipId;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const PREFIX: &str = "cloud:";

pub fn key(id: ClipId, file_name: &str) -> String {
    format!("{PREFIX}{id}/{file_name}")
}

pub fn is_cloud_key(path: &str) -> bool {
    path.starts_with(PREFIX)
}

pub fn parse_key(path: &str) -> Option<ClipId> {
    let rest = path.strip_prefix(PREFIX)?;
    ClipId::parse(rest.split('/').next()?)
}

/// Where each cloud-only clip can be fetched from (kept by the service as the cloud indexes change; read by
/// the player's worker thread).
static DIRECTORY: LazyLock<Mutex<HashMap<ClipId, (AccountSettings, String)>>> = LazyLock::new(Default::default);

pub(super) fn set_directory(entries: HashMap<ClipId, (AccountSettings, String)>) {
    *DIRECTORY.lock().unwrap() = entries;
}

/// What the player opens for a cloud key: a video streams from the cloud (blocking network: call it off the
/// UI thread); None if the clip is unknown or the cloud cannot be reached.
pub fn resolve(path: &str) -> Option<StreamSource> {
    let id = parse_key(path)?;
    let (account, remote_id) = DIRECTORY.lock().unwrap().get(&id).cloned()?;
    match providers::open(&account).and_then(|p| p.stream(&remote_id)) {
        Ok(source) => Some(source),
        Err(e) => {
            log::warn!("[Cloud] cannot stream {id}: {e}");
            None
        }
    }
}

/// A cloud-only screenshot, fetched into a temporary file the player can decode.
pub fn fetch_image(path: &str) -> Option<std::path::PathBuf> {
    let id = parse_key(path)?;
    let (account, remote_id) = DIRECTORY.lock().unwrap().get(&id).cloned()?;
    let name = path.rsplit('/').next().unwrap_or("image.png");
    let target = std::env::temp_dir().join(format!("lag-cloud-{id}-{name}"));
    if target.exists() {
        return Some(target);
    }
    let provider = providers::open(&account).ok()?;
    let mut bytes = Vec::new();
    provider.download(&remote_id, &mut bytes, &mut |_| true).ok()?;
    std::fs::write(&target, bytes).ok()?;
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_round_trip_and_keep_the_extension() {
        let id = ClipId::new_random();
        let k = key(id, "2026-11-20_00-19-35-Replay.mp4");
        assert!(is_cloud_key(&k) && k.ends_with(".mp4"));
        assert_eq!(parse_key(&k), Some(id));
        assert_eq!(parse_key(r"D:\Lag\a.mp4"), None);
        assert!(resolve(&key(ClipId::new_random(), "x.mp4")).is_none(), "unknown clip");
    }
}
