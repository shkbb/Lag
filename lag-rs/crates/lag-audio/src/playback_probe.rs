//! Mirrors Services/VfrCapture/PlaybackTrackProbe.cs (LAG_MFPROBE), without creating a player/window.
use crate::{wasapi::Apartment, Result};
use windows::{
    Foundation::Uri,
    Media::{Core::MediaSource, Playback::MediaPlaybackItem},
};
#[derive(Debug)]
pub struct PlaybackAudioTrack {
    pub label: String,
    pub language: String,
    pub id: String,
    pub subtype: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate: u32,
    pub encoding_available: bool,
}
#[derive(Debug)]
pub struct PlaybackTrackReport {
    pub selected_index: i32,
    pub tracks: Vec<PlaybackAudioTrack>,
}
pub fn probe(path: &str) -> Result<PlaybackTrackReport> {
    let _com = Apartment::mta()?;
    // WinRT requires RoInitialize in addition to COM. Balanced even if media opening fails.
    let _runtime = Runtime::new()?;
    let url = if path.contains("://") {
        path.to_owned()
    } else {
        let absolute = std::fs::canonicalize(path)?;
        let text = absolute.to_string_lossy();
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/");
        let network = text.starts_with("UNC/") || text.starts_with("//");
        let text = text.strip_prefix("UNC/").unwrap_or(&text).trim_start_matches('/');
        let mut escaped = String::new();
        for b in text.bytes() {
            if b.is_ascii_alphanumeric() || b"/:._-~".contains(&b) {
                escaped.push(b as char);
            } else {
                use std::fmt::Write;
                let _ = write!(escaped, "%{b:02X}");
            }
        }
        if network {
            format!("file://{escaped}")
        } else {
            format!("file:///{escaped}")
        }
    };
    let uri = Uri::CreateUri(&windows::core::HSTRING::from(url))?;
    let source = MediaSource::CreateFromUri(&uri)?;
    let result = (|| -> Result<PlaybackTrackReport> {
        source.OpenAsync()?.join()?;
        let item = MediaPlaybackItem::Create(&source)?;
        let list = item.AudioTracks()?;
        let selected_index = list.SelectedIndex()?;
        let mut tracks = Vec::new();
        for i in 0..list.Size()? {
            let t = list.GetAt(i)?;
            let encoding = (|| -> windows::core::Result<_> {
                let p = t.GetEncodingProperties()?;
                Ok((p.Subtype()?.to_string(), p.SampleRate()?, p.ChannelCount()?, p.Bitrate()?))
            })()
            .ok();
            let encoding_available = encoding.is_some();
            let (subtype, sample_rate, channels, bitrate) = encoding.unwrap_or_default();
            tracks.push(PlaybackAudioTrack {
                label: t.Label()?.to_string(),
                language: t.Language()?.to_string(),
                id: t.Id()?.to_string(),
                subtype,
                sample_rate,
                channels,
                bitrate,
                encoding_available,
            });
        }
        Ok(PlaybackTrackReport { selected_index, tracks })
    })();
    let _ = source.Close();
    result
}
pub fn run_from_env() {
    for path in std::env::var("LAG_MFPROBE").unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
        log::info!("[MfProbe] {}", std::path::Path::new(path).file_name().unwrap_or_default().to_string_lossy());
        match probe(path) {
            Ok(r) => {
                log::info!(
                    "[MfProbe]   audio tracks: {}, selected by default: index {}",
                    r.tracks.len(),
                    r.selected_index
                );
                for (i, t) in r.tracks.iter().enumerate() {
                    let encoding = if t.encoding_available {
                        format!("{} {}Hz {}ch {}kbps", t.subtype, t.sample_rate, t.channels, t.bitrate / 1000)
                    } else {
                        String::new()
                    };
                    log::info!(
                        "[MfProbe]   [{i}] {} label='{}' lang='{}' id='{}' {}",
                        if i as i32 == r.selected_index { "SELECTED" } else { "        " },
                        t.label,
                        t.language,
                        t.id,
                        encoding
                    );
                }
            }
            Err(e) => log::warn!("[MfProbe]   failed: {e}"),
        }
    }
}
struct Runtime;
impl Runtime {
    fn new() -> Result<Self> {
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
    }
}
