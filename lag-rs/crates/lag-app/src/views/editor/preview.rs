//! What the live preview asks mpv for: audio track routing, colour/crop/rotation as a GPU shader, flips.

use super::{export::*, model::*};

pub fn select_preview_audio(tracks: &[AudioTrack]) -> (bool, usize, i32) {
    let unmuted: Vec<&AudioTrack> = tracks.iter().filter(|t| !t.is_muted && t.volume > 0).collect();
    if unmuted.is_empty() {
        return (true, 0, 0);
    }
    let play_stream = if unmuted.len() == 1 {
        unmuted[0].stream_index
    } else {
        let has_mix_track = tracks.iter().all(|t| t.stream_index >= 1);
        if has_mix_track {
            0
        } else {
            unmuted[0].stream_index
        }
    };
    let vol = unmuted.iter().map(|t| t.volume).max().unwrap_or(100).clamp(0, 100);
    (false, play_stream, vol)
}

/// Preview "look" on the GPU (owner, 2026-11-17: "filters and other heavy functions sometimes lag"): colour
/// presets + colour sliders = one mpv user shader whose parameters change (glsl-shader-opts, no recompile, no
/// filter restart); crop = mpv `video-crop`, rotation = `video-rotate` (both in the renderer). Only the flips
/// stay a (CPU) vf chain. Export still uses the exact ffmpeg filters (build_video_filter_chain).
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewLook {
    pub vf: String,
    pub shader_opts: String,
    /// normalized crop of the FLIPPED frame (vf flips run before video-crop), None = whole frame
    pub crop: Option<(f64, f64, f64, f64)>,
    pub rotate: i32,
}

/// ffmpeg vf_colortemperature's kelvin -> RGB multipliers (mix=1, no lightness preservation).
pub fn kelvin_rgb(kelvin: f64) -> (f64, f64, f64) {
    let t = kelvin / 100.0;
    let (r, g) = if t <= 66.0 {
        (1.0, 0.390_081_578_769_019_6 * t.ln() - 0.631_841_443_788_627_4)
    } else {
        (
            1.292_936_186_062_745 * (t - 60.0).powf(-0.133_204_759_2),
            1.129_890_860_895_294 * (t - 60.0).powf(-0.075_514_849_2),
        )
    };
    let b = if t >= 66.0 {
        1.0
    } else if t <= 19.0 {
        0.0
    } else {
        0.543_206_789_110_196 * (t - 10.0).ln() - 1.196_254_089_14
    };
    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

pub fn build_preview_look(model: &EditModel) -> PreviewLook {
    // preset: (sepia, temperature multipliers, eq contrast/brightness/saturation) mirroring default_filter_options
    let (mut sepia, mut temp, mut eq1) = (0.0, (1.0, 1.0, 1.0), (1.0, 0.0, 1.0));
    match model.selected_filter().name {
        "Grayscale" => eq1 = (1.0, 0.0, 0.0),
        "Sepia" => sepia = 1.0,
        "Warm" => temp = kelvin_rgb(4500.0),
        "Cold" => temp = kelvin_rgb(8500.0),
        "Vibrant" => eq1 = (1.05, 0.0, 1.6),
        "Dramatic" => eq1 = (1.35, -0.06, 0.8),
        "Bright" => eq1 = (1.0, 0.08, 1.15),
        _ => {}
    }
    let eq2 = (
        model.color_contrast as f64 / 100.0,
        model.color_brightness as f64 / 100.0 - 1.0,
        model.color_saturation as f64 / 100.0,
    );
    let shader_opts = format!(
        "lag_sepia={},lag_tr={},lag_tg={},lag_tb={},lag_c1={},lag_b1={},lag_s1={},lag_c2={},lag_b2={},lag_s2={}",
        fmt_dec(sepia, 1),
        fmt_dec(temp.0, 4),
        fmt_dec(temp.1, 4),
        fmt_dec(temp.2, 4),
        fmt_dec(eq1.0, 3),
        fmt_dec(eq1.1, 3),
        fmt_dec(eq1.2, 3),
        fmt_dec(eq2.0, 3),
        fmt_dec(eq2.1, 3),
        fmt_dec(eq2.2, 3)
    );
    let mut flips = Vec::new();
    if model.flip_h {
        flips.push("hflip");
    }
    if model.flip_v {
        flips.push("vflip");
    }
    let crop = (!model.crop_tool_active && model.is_cropped()).then(|| {
        let (mut x, mut y) = (model.crop_x, model.crop_y);
        if model.flip_h {
            x = 1.0 - model.crop_x - model.crop_w;
        }
        if model.flip_v {
            y = 1.0 - model.crop_y - model.crop_h;
        }
        (x, y, model.crop_w, model.crop_h)
    });
    PreviewLook { vf: flips.join(","), shader_opts, crop, rotate: model.rotation.rem_euclid(360) }
}

/// The editor preview's user shader (see PreviewLook); loaded once, driven by glsl-shader-opts.
pub const PREVIEW_SHADER: &str = r#"//!PARAM lag_sepia
//!TYPE float
0.0

//!PARAM lag_tr
//!TYPE float
1.0

//!PARAM lag_tg
//!TYPE float
1.0

//!PARAM lag_tb
//!TYPE float
1.0

//!PARAM lag_c1
//!TYPE float
1.0

//!PARAM lag_b1
//!TYPE float
0.0

//!PARAM lag_s1
//!TYPE float
1.0

//!PARAM lag_c2
//!TYPE float
1.0

//!PARAM lag_b2
//!TYPE float
0.0

//!PARAM lag_s2
//!TYPE float
1.0

//!HOOK MAIN
//!BIND HOOKED
//!DESC Lag editor look

vec3 lag_eq(vec3 rgb, float c, float b, float s) {
    float y = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3 chroma = rgb - vec3(y);
    return vec3((y - 0.5) * c + 0.5 + b) + chroma * s;
}

vec4 hook() {
    vec4 col = HOOKED_texOff(0);
    vec3 rgb = col.rgb;
    if (lag_sepia > 0.5) {
        rgb = vec3(dot(rgb, vec3(0.393, 0.769, 0.189)),
                   dot(rgb, vec3(0.349, 0.686, 0.168)),
                   dot(rgb, vec3(0.272, 0.534, 0.131)));
    }
    rgb *= vec3(lag_tr, lag_tg, lag_tb);
    rgb = lag_eq(rgb, lag_c1, lag_b1, lag_s1);
    rgb = lag_eq(rgb, lag_c2, lag_b2, lag_s2);
    col.rgb = clamp(rgb, 0.0, 1.0);
    return col;
}
"#;

#[allow(dead_code)] // the full CPU chain: kept as the reference the GPU look mirrors (tests)
pub fn build_mpv_vf(model: &EditModel) -> String {
    let mut parts = Vec::new();
    if let Some(graph) = model.selected_filter().graph {
        parts.push(graph.to_string());
    }
    if model.has_color_adjust() {
        let c = model.color_contrast as f64 / 100.0;
        let b = model.color_brightness as f64 / 100.0 - 1.0;
        let s = model.color_saturation as f64 / 100.0;
        parts.push(format!("eq=contrast={}:brightness={}:saturation={}", fmt_dec(c, 3), fmt_dec(b, 3), fmt_dec(s, 3)));
    }
    if !model.crop_tool_active && model.is_cropped() {
        parts.push(format!(
            "crop=iw*{}:ih*{}:iw*{}:ih*{}",
            fmt_dec(model.crop_w, 5),
            fmt_dec(model.crop_h, 5),
            fmt_dec(model.crop_x, 5),
            fmt_dec(model.crop_y, 5)
        ));
    }
    if model.flip_h {
        parts.push("hflip".to_string());
    }
    if model.flip_v {
        parts.push("vflip".to_string());
    }
    match model.rotation {
        90 => parts.push("transpose=1".to_string()),
        180 => {
            parts.push("transpose=1".to_string());
            parts.push("transpose=1".to_string());
        }
        270 => parts.push("transpose=2".to_string()),
        _ => {}
    }
    parts.join(",")
}
