//! Mirrors Services/PresetResolver.cs: pure hardware classification and recommendation tables.

use crate::hardware::MachineProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingIntent {
    Performance,
    Balanced,
    Quality,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GpuTier {
    Entry,
    Mid,
    High,
    Top,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendedSettings {
    pub target_height: i32,
    pub fps: i32,
    pub codec_id: String,
    pub bitrate_kbps: i32,
    pub buffer_seconds: i32,
    /// Hint only. Never automatically apply cleanup, which deletes clips.
    pub recommend_cleanup: bool,
    pub suggested_storage_gb: i32,
}
impl RecommendedSettings {
    pub fn summary(&self) -> String {
        let res = if self.target_height == 0 { "native".into() } else { format!("{}p", self.target_height) };
        let codec = if self.codec_id.is_empty() { "Auto codec".into() } else { self.codec_id.to_uppercase() };
        let rate =
            if self.bitrate_kbps < 0 { "Auto quality".into() } else { format!("{} Mbps", self.bitrate_kbps / 1000) };
        let buf = if self.buffer_seconds < 60 {
            format!("{}s", self.buffer_seconds)
        } else {
            format!("{}min", self.buffer_seconds / 60)
        };
        format!("{res} · {}fps · {codec} · {rate} · {buf} buffer", self.fps)
    }
}
pub fn classify_gpu(p: &MachineProfile) -> GpuTier {
    use GpuTier::*;
    let Some(gpu) = &p.primary_gpu else {
        return Entry;
    };
    if gpu.is_integrated() {
        return Entry;
    }
    let v = gpu.vram_gib();
    let mut tier = if v >= 12.0 {
        Top
    } else if v >= 8.0 {
        High
    } else if v >= 4.0 {
        Mid
    } else {
        Entry
    };
    let n = gpu.name.to_uppercase();
    let nv = n
        .split([' ', '(', ')'])
        .find_map(|t| {
            if t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit()) {
                t.parse::<i32>().ok().filter(|m| *m >= 1000).map(|m| m % 100)
            } else {
                None
            }
        })
        .unwrap_or(0);
    if n.contains("RTX 50") || n.contains("RTX 40") {
        tier = if nv >= 70 { Top } else { High };
    } else if n.contains("RTX 30") {
        tier = if nv >= 80 { Top } else { High };
    } else if ["RTX 20", "GTX 16", "GTX 10"].iter().any(|s| n.contains(s)) {
        tier = tier.min(Mid);
    } else if n.contains("RX 9") || n.contains("RX 7") {
        tier = if n.contains("900") || n.contains("800") { Top } else { High };
    } else if n.contains("RX 6") {
        tier = if n.contains("6900") || n.contains("6800") {
            Top
        } else if n.contains("6700") {
            High
        } else {
            Mid
        };
    } else if ["RX 5", "RX 4", "RX 580", "RX 570"].iter().any(|s| n.contains(s)) {
        tier = tier.min(Mid);
    }
    tier
}

pub const BUFFER_RUNGS: &[i32] = &[30, 60, 120, 300, 600, 900];
/// Webcam overlay modes (settings `WebcamMode` index): (width, height, fps); (0, 0, 0) = Auto = a modest
/// ~640-wide mode at ~30 fps (the picture-in-picture is small). A camera without the exact mode streams the
/// nearest one it offers (owner, 2026-11-18: "webcam modes like 30/60 fps, different resolutions, presets").
pub const WEBCAM_MODES: &[(u32, u32, u32)] =
    &[(0, 0, 0), (640, 480, 30), (1280, 720, 30), (1280, 720, 60), (1920, 1080, 30), (1920, 1080, 60)];
/// (width, height, fps) of a `WebcamMode` index; out of range = Auto.
pub fn webcam_mode(index: i32) -> (u32, u32, u32) {
    usize::try_from(index).ok().and_then(|i| WEBCAM_MODES.get(i)).copied().unwrap_or((0, 0, 0))
}
pub const BITRATE_RUNGS: &[i32] = &[3000, 5000, 7000, 10000, 15000, 20000, 25000, 30000, 50000, 70000, 100000];
pub const STORAGE_RUNGS: &[i32] = &[10, 25, 50, 100, 200, 500];
fn nearest(value: i32, rungs: &[i32]) -> i32 {
    rungs.iter().copied().min_by_key(|r| (i64::from(*r) - i64::from(value)).abs()).unwrap_or(0)
}
fn snap_resolution(wanted: i32, monitor: i32) -> i32 {
    if wanted <= 0 || wanted >= monitor {
        return 0;
    }
    [2160, 1440, 1080, 720].into_iter().find(|r| *r <= monitor && wanted >= *r).unwrap_or(720)
}
fn quality_bitrate(h: i32) -> i32 {
    if h >= 2160 {
        100000
    } else if h >= 1440 {
        50000
    } else if h >= 1080 {
        30000
    } else {
        15000
    }
}
fn auto_estimate(h: i32) -> i32 {
    if h >= 2160 {
        70000
    } else if h >= 1440 {
        38000
    } else if h >= 1080 {
        20000
    } else {
        9000
    }
}

pub fn resolve(intent: RecordingIntent, p: &MachineProfile) -> RecommendedSettings {
    use GpuTier::*;
    use RecordingIntent::*;
    let tier = classify_gpu(p);
    let cpu_only = p.hardware_encoders.is_empty();
    let mon_h = if p.monitor_height > 0 { p.monitor_height } else { 1080 };
    let refresh = if p.monitor_hz > 0 { p.monitor_hz } else { 60 };
    let mut height = match intent {
        Performance => {
            if tier >= Top {
                1440
            } else {
                1080
            }
        }
        Balanced => {
            if tier >= Top {
                0
            } else if tier >= High {
                1440
            } else {
                1080
            }
        }
        Quality => {
            if tier >= High {
                0
            } else if tier == Mid {
                1440
            } else {
                1080
            }
        }
    };
    if cpu_only {
        height = if p.cpu_logical_cores >= 8 { 1080 } else { 720 };
    }
    let target_height = snap_resolution(height, mon_h);
    let mut fps = match intent {
        Performance => {
            if tier >= Mid {
                120
            } else {
                60
            }
        }
        Balanced => {
            if tier >= High {
                120
            } else {
                60
            }
        }
        Quality => {
            if tier >= Top {
                120
            } else {
                60
            }
        }
    };
    if cpu_only {
        fps = 60;
    }
    fps = fps.min(refresh);
    let codec_id = match intent {
        Performance => {
            if p.has_h264 {
                "h264"
            } else {
                ""
            }
        }
        Balanced => "",
        Quality => {
            if p.has_av1 {
                "av1"
            } else if p.has_hevc {
                "hevc"
            } else if p.has_h264 {
                "h264"
            } else {
                ""
            }
        }
    }
    .into();
    let h = if target_height == 0 { mon_h } else { target_height };
    let mut bitrate_kbps = if intent == Quality { quality_bitrate(h) } else { -1 };
    let ram_bytes = (p.ram_gib * 1024.0 * 1024.0 * 1024.0) as i64;
    let budget = (ram_bytes as f64 * 0.15).clamp((250 * 1024 * 1024) as f64, (1500 * 1024 * 1024) as f64) as i64;
    let mut effective = if bitrate_kbps > 0 { bitrate_kbps } else { auto_estimate(h) };
    if bitrate_kbps > 0 && i64::from(bitrate_kbps) * 1000 / 8 * 30 > budget {
        bitrate_kbps = nearest((budget * 8 / 30 / 1000) as i32, BITRATE_RUNGS);
        effective = bitrate_kbps;
    }
    let target = match intent {
        Performance => 60,
        Balanced => 120,
        Quality => 180,
    };
    let max_seconds = budget * 8 / (i64::from(effective) * 1000).max(1);
    let buffer_seconds = nearest(i64::from(target).min(max_seconds.max(30)) as i32, BUFFER_RUNGS);
    let recommend_cleanup = p.disk_free_gib > 0.0 && p.disk_free_gib < 40.0;
    let suggested_storage_gb =
        if recommend_cleanup { nearest((p.disk_free_gib * 0.6) as i32, STORAGE_RUNGS) } else { 0 };
    RecommendedSettings {
        target_height,
        fps,
        codec_id,
        bitrate_kbps,
        buffer_seconds,
        recommend_cleanup,
        suggested_storage_gb,
    }
}

pub fn log_recommendations(p: &MachineProfile) {
    log::info!("===== Preset recommendations (GPU tier: {:?}) =====", classify_gpu(p));
    for intent in [RecordingIntent::Performance, RecordingIntent::Balanced, RecordingIntent::Quality] {
        let r = resolve(intent, p);
        let hint = if r.recommend_cleanup {
            format!("  [hint: suggest auto-cleanup ~{}GB]", r.suggested_storage_gb)
        } else {
            String::new()
        };
        log::info!("  {:<12}: {}{}", format!("{intent:?}"), r.summary(), hint);
    }
    log::info!("==========================================");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::GpuProfile;
    fn profile(name: &str, vram: i64) -> MachineProfile {
        MachineProfile {
            primary_gpu: Some(GpuProfile {
                name: name.into(),
                dedicated_vram_bytes: vram * 1024 * 1024 * 1024,
                ..Default::default()
            }),
            hardware_encoders: vec!["HW".into()],
            has_h264: true,
            has_hevc: true,
            has_av1: true,
            ram_gib: 32.0,
            monitor_height: 2160,
            monitor_hz: 144,
            cpu_logical_cores: 8,
            ..Default::default()
        }
    }
    #[test]
    fn csharp_gpu_classification_table() {
        use GpuTier::*;
        for (name, vram, expected) in [
            ("Iris Xe", 0, Entry),
            ("unknown", 2, Entry),
            ("unknown", 4, Mid),
            ("unknown", 6, Mid),
            ("Arc A770", 8, High),
            ("Arc A770", 16, Top),
            ("RTX 4090", 24, Top),
            ("RTX 4070 Ti", 12, Top),
            ("RTX 4060", 8, High),
            ("RTX 5060", 8, High),
            ("RTX 5070", 12, Top),
            ("RTX 3080", 10, Top),
            ("RTX 3070", 8, High),
            ("RTX 3060", 12, High),
            ("RTX 2080", 8, Mid),
            ("GTX 1660", 6, Mid),
            ("GTX 1080", 8, Mid),
            ("RX 7900", 24, Top),
            ("RX 7800", 16, Top),
            ("RX 7600", 8, High),
            ("RX 9070", 16, High),
            ("RX 6900", 16, Top),
            ("RX 6800", 16, Top),
            ("RX 6700", 12, High),
            ("RX 6600", 8, Mid),
            ("RX 580", 8, Mid),
            ("RX 480", 8, Mid),
        ] {
            assert_eq!(classify_gpu(&profile(name, vram)), expected, "{name}");
        }
    }
    #[test]
    fn csharp_intent_cross_tier_table() {
        use RecordingIntent::*;
        // Native 2160p, 144 Hz, ample RAM: columns performance, balanced, quality.
        for (name, vram, heights, fps, rates, buffers) in [
            ("iGPU", 0, [1080, 1080, 1080], [60, 60, 60], [-1, -1, 30000], [60, 120, 120]),
            ("RX 580", 8, [1080, 1080, 1440], [120, 60, 60], [-1, -1, 50000], [60, 120, 120]),
            ("RTX 4060", 8, [1080, 1440, 0], [120, 120, 60], [-1, -1, 100000], [60, 120, 120]),
            ("RTX 4070", 12, [1440, 0, 0], [120, 120, 120], [-1, -1, 100000], [60, 120, 120]),
        ] {
            for (i, intent) in [Performance, Balanced, Quality].into_iter().enumerate() {
                let r = resolve(intent, &profile(name, vram));
                assert_eq!(
                    (r.target_height, r.fps, r.bitrate_kbps, r.buffer_seconds),
                    (heights[i], fps[i], rates[i], buffers[i]),
                    "{name} {intent:?}"
                );
                assert_eq!(r.codec_id, ["h264", "", "av1"][i]);
            }
        }
    }
    #[test]
    fn rungs_limits_and_fallbacks() {
        assert_eq!(BUFFER_RUNGS, [30, 60, 120, 300, 600, 900]);
        assert_eq!(BITRATE_RUNGS, [3000, 5000, 7000, 10000, 15000, 20000, 25000, 30000, 50000, 70000, 100000]);
        assert_eq!(STORAGE_RUNGS, [10, 25, 50, 100, 200, 500]);
        assert_eq!(nearest(180, BUFFER_RUNGS), 120); // Preserve C# nearest, even when above budget.
        assert_eq!(nearest(45, BUFFER_RUNGS), 30);
        for (h, q, a) in [(720, 15000, 9000), (1080, 30000, 20000), (1440, 50000, 38000), (2160, 100000, 70000)] {
            assert_eq!((quality_bitrate(h), auto_estimate(h)), (q, a));
        }
        let mut p = profile("RTX 4090", 24);
        p.hardware_encoders.clear();
        p.cpu_logical_cores = 4;
        p.monitor_hz = 50;
        p.disk_free_gib = 20.0;
        let r = resolve(RecordingIntent::Quality, &p);
        assert_eq!((r.target_height, r.fps, r.suggested_storage_gb), (720, 50, 10));
        p.cpu_logical_cores = 8;
        assert_eq!(resolve(RecordingIntent::Balanced, &p).target_height, 1080);
        p.monitor_height = 720;
        assert_eq!(resolve(RecordingIntent::Quality, &p).target_height, 0);
    }

    #[test]
    fn csharp_resolution_and_resource_edges() {
        for (wanted, monitor, expected) in [
            (0, 2160, 0),
            (2160, 2160, 0),
            (1440, 1080, 0),
            (1440, 2160, 1440),
            (1080, 1440, 1080),
            (720, 900, 720),
            (600, 1080, 720),
        ] {
            assert_eq!(snap_resolution(wanted, monitor), expected);
        }
        let mut p = profile("RTX 4090", 24);
        p.ram_gib = 0.0; // 250 MiB floor, C# snaps the bitrate to 70 Mbps (even if slightly over).
        let r = resolve(RecordingIntent::Quality, &p);
        assert_eq!((r.bitrate_kbps, r.buffer_seconds), (70000, 30));
        p.has_av1 = false;
        assert_eq!(resolve(RecordingIntent::Quality, &p).codec_id, "hevc");
        p.has_hevc = false;
        assert_eq!(resolve(RecordingIntent::Quality, &p).codec_id, "h264");
        p.has_h264 = false;
        assert_eq!(resolve(RecordingIntent::Performance, &p).codec_id, "");
        assert_eq!(resolve(RecordingIntent::Quality, &p).codec_id, "");
        for (free, cleanup, storage) in [(0.0, false, 0), (1.0, true, 10), (39.0, true, 25), (40.0, false, 0)] {
            p.disk_free_gib = free;
            let r = resolve(RecordingIntent::Balanced, &p);
            assert_eq!((r.recommend_cleanup, r.suggested_storage_gb), (cleanup, storage));
        }
        p.monitor_height = 0;
        p.monitor_hz = 0;
        let r = resolve(RecordingIntent::Balanced, &p);
        assert_eq!((r.target_height, r.fps), (0, 60));
    }
}
