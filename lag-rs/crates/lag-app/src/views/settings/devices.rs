//! What this PC has (monitors, encoders, microphones, outputs, webcams), probed on a worker thread.

use super::*;
use lag_core::{
    hardware::{self, CodecTier, EncoderCapability, MachineProfile, MicrophoneInfo, MonitorInfo},
    settings::SettingsData,
};
use std::path::Path;

pub(super) struct Hardware {
    pub(super) monitors: Vec<MonitorInfo>,
    pub(super) mics: Vec<MicrophoneInfo>,
    pub(super) outputs: Vec<MicrophoneInfo>,
    pub(super) webcams: Vec<lag_capture::overlay::webcam::DeviceInfo>,
    pub(super) gpus: Vec<hardware::GpuInfo>,
    pub(super) codecs: Vec<(String, String)>,
    pub(super) profile: MachineProfile,
}
impl Hardware {
    pub(super) fn probe(s: &SettingsData) -> Self {
        let caps: Vec<_> = lag_capture::encoder::available()
            .iter()
            .map(|e| EncoderCapability {
                friendly_name: e.friendly_name.into(),
                is_hardware: e.is_hardware,
                tier: match e.tier {
                    lag_capture::encoder::CodecTier::H264 => CodecTier::H264,
                    lag_capture::encoder::CodecTier::Hevc => CodecTier::Hevc,
                    lag_capture::encoder::CodecTier::Av1 => CodecTier::Av1,
                },
            })
            .collect();
        let mut codecs = vec![(String::new(), tr("Option_Auto"))];
        for (tier, id, name) in [
            (CodecTier::H264, "h264", "H.264"),
            (CodecTier::Hevc, "hevc", "H.265 (HEVC)"),
            (CodecTier::Av1, "av1", "AV1"),
        ] {
            if caps.iter().any(|e| e.tier == tier) {
                codecs.push((id.into(), name.into()));
            }
        }
        if codecs.len() == 1 {
            codecs.push(("h264".into(), "H.264".into()));
        }
        let mut outputs = vec![MicrophoneInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
        outputs.extend(hardware::get_output_devices());
        // Mic and webcam: an empty id = follow the Windows default device (owner decision 2026-11-16:
        // a saved device that is not connected falls back to the Windows default, not to the first one).
        let mut mics = vec![MicrophoneInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
        mics.extend(hardware::get_microphones().into_iter().filter(|m| m.id != "default"));
        let mut webcams =
            vec![lag_capture::overlay::webcam::DeviceInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
        webcams.extend(lag_capture::overlay::webcam::WebcamCaptureSource::list_devices());
        Self {
            monitors: hardware::get_available_monitors(),
            mics,
            outputs,
            webcams,
            gpus: hardware::get_gpu_adapters(),
            codecs,
            profile: MachineProfile::capture(s.library_path.as_deref().map(Path::new), &caps),
        }
    }
}
