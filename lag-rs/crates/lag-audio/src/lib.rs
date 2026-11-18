//! Audio subsystem port of Services/VfrCapture/{WasapiAudioSource,ProcessLoopbackCapture,
//! AacAudioEncoder,NoiseGate,MicDenoiser}.cs and Services/Audio*.cs.
pub mod aac;
pub mod clock;
pub mod denoiser;
pub mod devices;
pub mod monitor;
pub mod noise_gate;
mod pcm;
pub mod playback_probe;
pub mod sessions;
pub mod source;
pub mod tracks;
pub mod wasapi;
pub use aac::AacAudioEncoder;
pub use denoiser::MicDenoiser;
pub use lag_ffi::media::{EncodedPacket, StreamSpec};
pub use noise_gate::NoiseGate;
pub use pcm::PcmFormat;
pub use source::{AudioControls, AudioSourceOptions, WasapiAudioSource};
pub use tracks::AudioTracks;
pub use wasapi::{ProcessLoopbackCapture, ProcessLoopbackMode};
#[derive(Debug, Clone)]
pub struct AudioError(pub String);
impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AudioError {}
impl From<windows_core::Error> for AudioError {
    fn from(e: windows_core::Error) -> Self {
        Self(e.to_string())
    }
}
impl From<std::io::Error> for AudioError {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, AudioError>;
fn check(code: i32, what: &str) -> Result<()> {
    if code < 0 {
        Err(AudioError(format!("{what}: {}", lag_ffi::av_err_string(code))))
    } else {
        Ok(())
    }
}
