use lag_player::PlayerEvent;
use std::{collections::VecDeque, sync::mpsc};

use super::{host::*, session::*, sidebar::*, worker::*};

#[test]
fn clock_matches_csharp() {
    assert_eq!(clock(0.0), "0:00.000");
    assert_eq!(clock(65.123), "1:05.123");
    assert_eq!(clock(3661.001), "1:01:01.001");
    assert_eq!(clock(-2.0), "0:00.000");
}

#[test]
fn drag_release_supersedes_pending_keyframe() {
    assert!(Command::Seek(0.8, true).supersedes(&Command::Seek(0.7, false)));
    assert!(!Command::Load { path: "next.mp4".into(), gen: 2 }.supersedes(&Command::Seek(0.7, false)));
}

#[test]
fn rapid_clip_switching_coalesces_obsolete_loads_and_seeks_preserving_volume() {
    let (tx, rx) = mpsc::channel();
    let _ = tx.send(Command::Seek(0.5, false));
    let _ = tx.send(Command::Volume(45.0));
    let _ = tx.send(Command::Load { path: "clip2.mp4".into(), gen: 2 });
    let _ = tx.send(Command::Seek(0.2, true));
    let _ = tx.send(Command::Load { path: "clip3.mp4".into(), gen: 3 });

    let mut pending = VecDeque::new();
    let first = coalesce_command_burst(Command::Load { path: "clip1.mp4".into(), gen: 1 }, &mut pending, &rx);
    assert_eq!(first, Command::Load { path: "clip3.mp4".into(), gen: 3 });
    let rest: Vec<_> = pending.into_iter().collect();
    assert_eq!(rest, vec![Command::Volume(45.0)]);
}

#[test]
fn clip_switch_resets_state_and_filters_stale_events() {
    let mut s = PlayerSessionState::default();
    let g1 = s.begin_load("a.mp4");
    assert_eq!(g1, 1);
    assert!(s.is_loading);
    assert!(!s.is_playing);
    assert_eq!(s.time_display(), "0:00.000 / 0:00.000");

    s.on_worker_load_started(g1);
    assert!(s.apply_event(g1, &PlayerEvent::Duration(20.0), false).applied);
    assert!(s.apply_event(g1, &PlayerEvent::FileLoaded, false).select_default_track);
    assert!(s.apply_event(g1, &PlayerEvent::Position(10.0), false).applied);
    assert!((s.norm_position - 0.5).abs() < 0.001);

    // Switch to clip B: resets position, duration, playing, loading.
    let g2 = s.begin_load("b.mp4");
    assert_eq!(g2, 2);
    assert!(s.is_loading);
    assert!(!s.is_playing);
    assert_eq!(s.position_sec, 0.0);
    assert_eq!(s.duration_sec, 0.0);
    assert_eq!(s.norm_position, 0.0);
    assert_eq!(s.time_display(), "0:00.000 / 0:00.000");

    // Stale position/stopped from clip A must be ignored.
    assert!(!s.apply_event(g1, &PlayerEvent::Position(11.0), false).applied);
    assert!(!s.apply_event(g1, &PlayerEvent::Stopped, false).applied);
    assert_eq!(s.position_sec, 0.0);

    // Even before FileLoaded for B, an old Stopped event emitted after worker started B is ignored.
    s.on_worker_load_started(g2);
    assert!(!s.apply_event(g2, &PlayerEvent::Stopped, false).applied);
    assert!(!s.apply_event(g2, &PlayerEvent::Position(12.0), false).applied);
    assert!(s.is_loading);

    // B loads and reports its own duration.
    assert!(s.apply_event(g2, &PlayerEvent::Duration(8.0), false).applied);
    assert!(s.apply_event(g2, &PlayerEvent::FileLoaded, false).applied);
    assert!(!s.is_loading);
    assert!(s.is_playing);
    assert_eq!(s.duration_sec, 8.0);
}

#[test]
fn switching_to_screenshot_clears_video_loading_and_time() {
    let mut s = PlayerSessionState::default();
    let g1 = s.begin_load("a.mp4");
    s.on_worker_load_started(g1);
    s.apply_event(g1, &PlayerEvent::Duration(12.0), false);
    s.apply_event(g1, &PlayerEvent::FileLoaded, false);

    let g2 = s.begin_load("shot.png");
    assert_ne!(g1, g2);
    assert!(s.is_image);
    assert!(!s.is_loading);
    assert!(!s.is_playing);
    assert_eq!(s.time_display(), "");
}

#[test]
fn logical_to_physical_rect_scales_and_insets() {
    assert_eq!(logical_to_physical_rect(102.0, 148.0, 900.0, 600.0, 1.25, 1.0), (129, 186, 1122, 748));
    assert_eq!(logical_to_physical_rect(0.0, 0.0, 1920.0, 1080.0, 1.0, 0.0), (0, 0, 1920, 1080));
    assert_eq!(logical_to_physical_rect(10.0, 10.0, 1.0, 1.0, 1.0, 1.0), (11, 11, 0, 0));
}

#[test]
fn surface_visibility_hides_when_inactive_minimized_transitioning_or_image() {
    let base = SurfaceVisibility {
        page_is_player: true,
        window_visible: true,
        window_minimized: false,
        fs_transition: false,
        has_clip: true,
        is_image: false,
        video_ready: true,
        width_px: 1280,
        height_px: 720,
    };
    assert!(!SurfaceVisibility { video_ready: false, ..base }.show_native());
    assert!(base.show_native());
    assert!(base.show_overlay());

    assert!(!SurfaceVisibility { page_is_player: false, ..base }.show_native());
    assert!(!SurfaceVisibility { page_is_player: false, ..base }.show_overlay());

    assert!(!SurfaceVisibility { window_minimized: true, ..base }.show_native());
    assert!(!SurfaceVisibility { window_minimized: true, ..base }.show_overlay());

    assert!(!SurfaceVisibility { window_visible: false, ..base }.show_native());
    assert!(!SurfaceVisibility { window_visible: false, ..base }.show_overlay());

    assert!(!SurfaceVisibility { fs_transition: true, ..base }.show_native());
    assert!(!SurfaceVisibility { fs_transition: true, ..base }.show_overlay());

    let img = SurfaceVisibility { is_image: true, ..base };
    assert!(!img.show_native());
    assert!(img.show_overlay());

    let no_clip = SurfaceVisibility { has_clip: false, ..base };
    assert!(!no_clip.show_native());
    assert!(no_clip.show_overlay());
}

#[test]
fn sidebar_totals_sum_durations_and_sizes() {
    let clips = vec![
        crate::app_ctx::SharedClip { duration_ticks: 90 * 10_000_000, file_size: 2_000_000, ..Default::default() },
        crate::app_ctx::SharedClip {
            duration_ticks: 30 * 10_000_000,
            file_size: 500_000,
            is_image: true,
            ..Default::default()
        },
    ];
    assert_eq!(sidebar_totals(&clips), (120, 2_500_000));
}
