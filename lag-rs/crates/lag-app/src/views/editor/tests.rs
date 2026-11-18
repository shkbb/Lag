use super::*;
use super::{export::*, geometry::*, model::*, preview::*, runtime::*};
use crate::services::ffmpeg;
use std::path::PathBuf;

#[test]
fn time_formatting_and_speed_parsing() {
    assert_eq!(format_time(0.0), "0:00");
    assert_eq!(format_time(65.9), "1:05");
    assert_eq!(format_time(3661.0), "1:01:01");
    assert_eq!(format_time(-5.0), "0:00");

    assert_eq!(parse_effective_speed("1"), 1.0);
    assert_eq!(parse_effective_speed(" 1,5 "), 1.5);
    assert_eq!(parse_effective_speed("0.01"), 0.1);
    assert_eq!(parse_effective_speed("25"), 10.0);
    assert_eq!(parse_effective_speed("bad"), 1.0);
}

#[test]
fn cut_merging_and_kept_segments() {
    let mut m = EditModel::new("Native");
    m.duration_sec = 20.0;
    m.trim_start_sec = 2.0;
    m.trim_end_sec = 18.0;

    m.add_cut(10.0, 12.0);
    m.add_cut(5.0, 7.0);
    m.add_cut(7.005, 8.0); // within 0.01 merge tolerance
    assert_eq!(m.cuts, vec![Cut { start_sec: 5.0, end_sec: 8.0 }, Cut { start_sec: 10.0, end_sec: 12.0 }]);

    let segs = m.kept_segments(2.0, 18.0);
    assert_eq!(segs, vec![(2.0, 5.0), (8.0, 10.0), (12.0, 18.0)]);

    assert_eq!(should_skip_cut(4.95, &m.cuts, 18.0), None);
    assert_eq!(should_skip_cut(4.98, &m.cuts, 18.0), Some(8.0));
    assert_eq!(should_skip_cut(10.5, &m.cuts, 11.0), Some(11.0));
}

#[test]
fn snapping_and_linked_trim_constraints() {
    let mut m = EditModel::new("Native");
    m.audio_follows_video = false; // the Shift-selected (C#) linking only
    m.duration_sec = 100.0;
    m.trim_start_sec = 0.0;
    m.trim_end_sec = 100.0;
    m.is_video_selected = true;
    let mut t = AudioTrack::new(0, "Audio", 100.0);
    t.start_sec = 25.0;
    t.end_sec = 75.0;
    t.is_selected = true;
    m.audio_tracks.push(t);

    // At width 1000px, threshold = 10/1000 * 100 = 1.0s; 24.4s snaps to 25.0s
    let seek = m.apply_timeline_drag(1, 0.244, 1000.0);
    assert_eq!(seek, Some(0.25));
    assert!((m.trim_start_sec - 25.0).abs() < 1e-6);
    assert!((m.audio_tracks[0].start_sec - 25.0).abs() < 1e-6);

    // Minimum trim gap is 0.2s
    m.apply_timeline_drag(1, 0.9999, 1000.0);
    assert!((m.trim_start_sec - 99.8).abs() < 1e-6);
    // Audio track maintains >= 0.1s gap from its 75.0s end
    assert!((m.audio_tracks[0].start_sec - 74.9).abs() < 1e-6);
}

#[test]
fn audio_follows_the_video_trim_unless_turned_off() {
    let mut m = EditModel::new("Native");
    assert!(m.audio_follows_video);
    m.duration_sec = 100.0;
    m.trim_start_sec = 0.0;
    m.trim_end_sec = 100.0;
    m.audio_tracks.push(AudioTrack::new(0, "Audio", 100.0));
    m.apply_timeline_drag(1, 0.2, 1000.0);
    m.apply_timeline_drag(2, 0.6, 1000.0);
    assert!((m.audio_tracks[0].start_sec - 20.0).abs() < 1e-6 && (m.audio_tracks[0].end_sec - 60.0).abs() < 1e-6);
    m.audio_follows_video = false;
    m.apply_timeline_drag(2, 0.8, 1000.0);
    assert!((m.trim_end_sec - 80.0).abs() < 1e-6 && (m.audio_tracks[0].end_sec - 60.0).abs() < 1e-6);
}

#[test]
fn undo_stack_restores_all_fields() {
    let mut m = EditModel::new("Native");
    m.reset_for_clip("C:\\clips\\test.mp4", 30.0, "Audio");
    m.source_width = 1920;
    m.source_height = 1080;

    m.push_undo();
    m.trim_start_sec = 5.0;
    m.trim_end_sec = 20.0;
    m.set_speed_text("2".to_string());
    m.selected_aspect_index = 2;
    m.selected_filter_index = 3;
    m.set_crop(0.1, 0.1, 0.5, 0.5);
    m.rotation = 90;
    m.flip_h = true;
    m.color_brightness = 130;
    m.add_cut(8.0, 12.0);
    m.texts.push(TextItem::new("Hello"));

    assert!(m.undo());
    assert_eq!(m.trim_start_sec, 0.0);
    assert_eq!(m.trim_end_sec, 30.0);
    assert_eq!(m.speed_text, "1");
    assert_eq!(m.selected_aspect_index, 0);
    assert_eq!(m.selected_filter_index, 0);
    assert_eq!(m.crop_w, 1.0);
    assert_eq!(m.rotation, 0);
    assert!(!m.flip_h);
    assert_eq!(m.color_brightness, 100);
    assert!(m.cuts.is_empty());
    assert!(m.texts.is_empty());
    assert!(!m.undo());
}

#[test]
fn rotation_and_flip_transforms_are_exact_inverses() {
    for flip_h in [false, true] {
        for flip_v in [false, true] {
            for rot in [0, 90, 180, 270] {
                let (dx, dy, dw, dh) = forward_transform_rect(0.12, 0.25, 0.45, 0.35, flip_h, flip_v, rot);
                let (x, y, w, h) = inverse_transform_rect(dx, dy, dw, dh, flip_h, flip_v, rot);
                assert!((x - 0.12).abs() < 1e-9);
                assert!((y - 0.25).abs() < 1e-9);
                assert!((w - 0.45).abs() < 1e-9);
                assert!((h - 0.35).abs() < 1e-9);
            }
        }
    }
}

#[test]
fn ffmpeg_video_filter_order_and_audio_mix_args() {
    let mut m = EditModel::new("Native");
    m.reset_for_clip("C:\\clips\\game.mp4", 30.0, "Audio");
    m.source_width = 1920;
    m.source_height = 1080;
    m.set_speed_text("4".to_string()); // chains atempo=2.0,atempo=2.0
    m.selected_filter_index = 5; // Vibrant
    m.color_brightness = 120;
    m.color_contrast = 110;
    m.color_saturation = 90;
    m.set_crop(0.1, 0.1, 0.8, 0.8);
    m.selected_aspect_index = 2; // 9:16
    m.reset_reframe();
    m.flip_h = true;
    m.rotation = 90;
    m.selected_export_res_index = 2; // 720p

    m.audio_tracks = vec![
        AudioTrack {
            stream_index: 1,
            name: "System".into(),
            volume: 80,
            is_muted: false,
            is_selected: false,
            start_sec: 5.0,
            end_sec: 20.0,
            clip_duration_sec: 30.0,
        },
        AudioTrack {
            stream_index: 2,
            name: "Mic".into(),
            volume: 100,
            is_muted: false,
            is_selected: false,
            start_sec: 0.0,
            end_sec: 30.0,
            clip_duration_sec: 30.0,
        },
    ];

    let txt = TextItem::new("Pog");
    let txt_path = PathBuf::from("C:\\Temp\\lag-txt-1.txt");
    let args = build_export_args(
        &m,
        "C:\\clips\\game.mp4",
        "C:\\clips\\game-edit.mp4",
        (2.0, 18.0),
        4.0,
        true,
        &[(&txt, txt_path.as_path())],
    );

    // Input -ss/-t BEFORE -i
    assert_eq!(&args[0..7], &["-y", "-ss", "2", "-t", "18", "-i", "C:\\clips\\game.mp4"]);

    let fc_idx = args.iter().position(|a| a == "-filter_complex").unwrap();
    let fc = &args[fc_idx + 1];

    // Exact video filter order
    let expected_substrings = [
        "setpts=PTS/4",
        "eq=saturation=1.6:contrast=1.05",
        "eq=contrast=1.1:brightness=0.2:saturation=0.9",
        "crop=iw*0.8:ih*0.8:iw*0.1:ih*0.1",
        "scale=",
        "hflip",
        "transpose=1",
        "scale=-2:720",
        "drawtext=fontfile='C\\:/Windows/Fonts/arialbd.ttf'",
    ];
    let mut last_pos = 0;
    for pat in expected_substrings {
        let pos =
            fc[last_pos..].find(pat).unwrap_or_else(|| panic!("missing `{pat}` after offset {last_pos} in `{fc}`"));
        last_pos += pos + pat.len();
    }

    // Audio chains & amix (speed 4x -> C# EditorViewModel.cs:1407-1415 chains "atempo=2.0,atempo=2")
    assert!(fc.contains("[0:a:1]volume=0.8,volume=0:enable='lt(t\\,3)+gt(t\\,18)',atempo=2.0,atempo=2[a1]"));
    assert!(fc.contains("[0:a:2]volume=1,atempo=2.0,atempo=2[a2]"));
    assert!(fc.contains("[a1][a2]amix=inputs=2:duration=longest:normalize=0[aout]"));

    assert!(args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
    assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
    assert!(args.windows(2).any(|w| w == ["-progress", "pipe:1"]));
}

#[test]
fn gif_export_args_and_progress_parsing() {
    let mut m = EditModel::new("Native");
    m.export_as_gif = true;
    let args = build_export_args(&m, "in.mp4", "out.gif", (1.5, 4.0), 1.0, true, &[]);
    let fc_idx = args.iter().position(|a| a == "-filter_complex").unwrap();
    assert_eq!(
        args[fc_idx + 1],
        "[0:v:0]fps=15,split[ga][gb];[ga]palettegen=max_colors=256[gp];[gb][gp]paletteuse=dither=sierra2_4a[v]"
    );
    assert!(args.contains(&"-an".to_string()));
    assert!(args.windows(2).any(|w| w == ["-loop", "0"]));
}

#[test]
fn ffprobe_json_parsing_and_audio_preview_routing() {
    let json = r#"{
        "streams": [
            { "codec_type": "video", "width": 2560, "height": 1440 },
            { "codec_type": "audio" },
            { "codec_type": "audio" },
            { "codec_type": "audio" }
        ],
        "format": { "duration": "42.750" }
    }"#;
    let res = ffmpeg::parse_ffprobe_json(json).unwrap();
    // the fields the editor uses (the full parse is tested in services::ffmpeg)
    assert_eq!((res.duration_sec, res.audio_streams, res.video_width, res.video_height), (42.75, 3, 2560, 1440));

    let tracks = vec![AudioTrack::new(1, "System", 42.75), AudioTrack::new(2, "Mic", 42.75)];
    // Both enabled -> pre-mixed stream 0
    assert_eq!(select_preview_audio(&tracks), (false, 0, 100));

    // Only mic enabled -> stream 2
    let mut tracks2 = tracks.clone();
    tracks2[0].is_muted = true;
    tracks2[1].volume = 65;
    assert_eq!(select_preview_audio(&tracks2), (false, 2, 65));

    // All muted -> mute true
    tracks2[1].is_muted = true;
    assert_eq!(select_preview_audio(&tracks2), (true, 0, 0));
}

#[test]
fn output_path_increments_in_temp_dir() {
    let dir = std::env::temp_dir().join(format!("lag-ed-test-{}", unique_hex_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("clip.mp4");
    std::fs::write(&src, b"").unwrap();
    let p1 = build_output_path(&src.to_string_lossy(), "mp4");
    assert_eq!(p1.file_name().unwrap(), "clip-edit.mp4");
    std::fs::write(&p1, b"").unwrap();
    let p2 = build_output_path(&src.to_string_lossy(), "mp4");
    assert_eq!(p2.file_name().unwrap(), "clip-edit2.mp4");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn vertical_export_scales_the_short_side() {
    assert_eq!(export_scale_filter(1080, true).as_deref(), Some("scale=1080:-2"));
    assert_eq!(export_scale_filter(1080, false).as_deref(), Some("scale=-2:1080"));
    assert_eq!(export_scale_filter(0, true), None);

    let mut m = EditModel::new("Native");
    m.source_width = 2580;
    m.source_height = 1080;
    m.selected_aspect_index = 2; // 9:16
    m.reset_reframe();
    assert!(m.is_portrait_output());
    m.selected_export_res_index = m.export_resolutions.iter().position(|r| r.height == 1080).unwrap();
    let chain = build_video_filter_chain(&m, 1.0, &[]).join(",");
    // the 21:9 clip is covered into a 608x1080 frame, then the export makes it 1080x1920
    assert!(chain.contains("pad=608:1080:"), "{chain}");
    assert!(chain.ends_with("scale=1080:-2"), "{chain}");
    // a 9:16 frame rotated 90 degrees is landscape again
    m.rotation = 90;
    assert!(!m.is_portrait_output());
}

#[test]
fn reframe_drag_snapping_clamping_and_crop_min_size() {
    let mut m = EditModel::new("Native");
    m.source_width = 1920;
    m.source_height = 1080;
    m.selected_aspect_index = 2; // 9:16
    m.reset_reframe();

    // Crop mode: enforces 0.05 min size and [0, 1] clamp
    m.crop_tool_active = true;
    let start_disp = m.display_edit_norm();
    apply_overlay_drag(&mut m, start_disp, 8, -2000.0, -2000.0, (100.0, 0.0, 400.0, 400.0), (600.0, 400.0));
    assert!((m.crop_w - 0.05).abs() < 1e-6);
    assert!((m.crop_h - 0.05).abs() < 1e-6);

    // Reframe mode: 9px snap to frame edge 0
    m.crop_tool_active = false;
    m.reset_crop();
    m.set_video_rect(0.0, 0.0, 0.5, 0.5);
    let start_disp = m.display_edit_norm();
    // refr_w = 400 -> 9px = 0.0225. Move right by 5px -> 5/400 = 0.0125 < 0.0225 -> snaps back to 0.0
    apply_overlay_drag(&mut m, start_disp, 1, 5.0, 0.0, (100.0, 0.0, 400.0, 400.0), (600.0, 400.0));
    assert!((m.vid_x - 0.0).abs() < 1e-6);
}

#[test]
fn playhead_interpolation_and_mpv_vf_builder() {
    let cuts = vec![Cut { start_sec: 4.0, end_sec: 6.0 }];
    // At 3.0s + 1.5s * 1.0 = 4.5s -> inside [4, 6), jumps to 6.0s -> 6/10 = 0.6
    let pos = interpolate_playhead(0.3, 1.5, 1.0, 10.0, &cuts, 1.0, 9.0);
    assert!((pos - 0.6).abs() < 1e-6);

    let mut m = EditModel::new("Native");
    m.selected_filter_index = 1; // Grayscale: hue=s=0
    m.color_brightness = 110;
    m.set_crop(0.1, 0.0, 0.8, 1.0);
    m.flip_h = true;
    m.rotation = 270;

    // When crop_tool_active is false, crop filter is included in mpv vf
    m.crop_tool_active = false;
    assert_eq!(
        build_mpv_vf(&m),
        "hue=s=0,eq=contrast=1:brightness=0.1:saturation=1,crop=iw*0.8:ih*1:iw*0.1:ih*0,hflip,transpose=2"
    );
    // When crop_tool_active is true, crop filter is omitted so full source is visible
    m.crop_tool_active = true;
    assert_eq!(build_mpv_vf(&m), "hue=s=0,eq=contrast=1:brightness=0.1:saturation=1,hflip,transpose=2");
}

#[test]
fn preview_look_moves_colour_crop_rotation_to_the_gpu() {
    let mut m = EditModel::new("Native");
    m.selected_filter_index = 1; // Grayscale
    m.color_brightness = 110;
    m.set_crop(0.1, 0.0, 0.8, 1.0);
    m.flip_h = true;
    m.rotation = 270;
    m.crop_tool_active = false;
    let look = build_preview_look(&m);
    assert_eq!(look.vf, "hflip");
    assert_eq!(look.rotate, 270);
    let (x, y, w, h) = look.crop.unwrap();
    assert!((x - 0.1).abs() < 1e-9 && y.abs() < 1e-9 && (w - 0.8).abs() < 1e-9 && (h - 1.0).abs() < 1e-9);
    assert!(look.shader_opts.contains("lag_s1=0,") && look.shader_opts.contains("lag_b2=0.1,"));
    m.crop_tool_active = true;
    assert_eq!(build_preview_look(&m).crop, None);
    // colortemperature multipliers (ffmpeg's kelvin formula)
    let (r, g, b) = kelvin_rgb(4500.0);
    assert!((r - 1.0).abs() < 1e-9 && (g - 0.8531).abs() < 1e-3 && (b - 0.7350).abs() < 1e-3);
    let (r, _, b) = kelvin_rgb(8500.0);
    assert!((r - 0.8423).abs() < 2e-3 && (b - 1.0).abs() < 1e-9);
    // mirrored crop for a flipped frame
    m.crop_tool_active = false;
    m.set_crop(0.0, 0.0, 0.5, 1.0);
    assert!((build_preview_look(&m).crop.unwrap().0 - 0.5).abs() < 1e-9);
}

#[test]
fn concat_args_and_reframe_passthrough() {
    let m = EditModel::new("Native");
    assert!(build_reframe_filters(&m).is_empty());

    let args = build_concat_args("C:\\temp\\list.txt", "C:\\out\\final.mp4", true);
    assert_eq!(
        args,
        vec![
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            "C:\\temp\\list.txt",
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            "C:\\out\\final.mp4"
        ]
    );
}

#[test]
fn editable_clips_filters_images_preserving_order() {
    let clips = vec![
        crate::app_ctx::SharedClip { file_path: "a.mp4".into(), is_image: false, ..Default::default() },
        crate::app_ctx::SharedClip { file_path: "shot.png".into(), is_image: true, ..Default::default() },
        crate::app_ctx::SharedClip { file_path: "b.mov".into(), is_image: false, ..Default::default() },
    ];
    let filtered: Vec<&str> = editable_clips(&clips).into_iter().map(|c| c.file_path.as_str()).collect();
    assert_eq!(filtered, vec!["a.mp4", "b.mov"]);
}
