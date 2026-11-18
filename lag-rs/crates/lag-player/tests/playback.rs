//! Headless parity checks for ViewModels/PlayerViewModel.cs transport and recording track order.
use lag_player::{MpvPlayer, PlayerEvent};
use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

struct Clip(PathBuf);
impl Clip {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "lag-player-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("three untitled tracks.mp4");
        let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../InstantReplay/ffmpeg/ffmpeg.exe");
        let mut command = Command::new(ffmpeg);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let output = command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=30:duration=4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=660:sample_rate=48000:duration=4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000:duration=4",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-map",
                "3:a",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "30",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "48k",
                "-movflags",
                "+faststart",
            ])
            .arg(&path)
            .output()
            .expect("bundled ffmpeg");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        Self(path)
    }
}
impl Drop for Clip {
    fn drop(&mut self) {
        let parent = self.0.parent().unwrap();
        // Only remove this test's freshly created directory, inside the OS temporary directory.
        let Ok(parent) = parent.canonicalize() else { return };
        let Ok(temp) = std::env::temp_dir().canonicalize() else { return };
        if parent.starts_with(temp) && parent.file_name().unwrap().to_string_lossy().starts_with("lag-player-test-") {
            for _ in 0..50 {
                if std::fs::remove_dir_all(&parent).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
fn event(rx: &mpsc::Receiver<PlayerEvent>, predicate: impl Fn(&PlayerEvent) -> bool) -> PlayerEvent {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let e = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("mpv event timeout");
        assert!(!matches!(&e, PlayerEvent::Error(_)), "{e:?}");
        if predicate(&e) {
            return e;
        }
    }
}
fn until(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(start.elapsed() < Duration::from_secs(10), "mpv property timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn number(p: &MpvPlayer, name: &str) -> f64 {
    p.property(name).unwrap().parse().unwrap()
}

#[test]
fn headless_duration_seeks_tracks_transport_and_eof() {
    let clip = Clip::new();
    let (tx, rx) = mpsc::channel();
    let player = MpvPlayer::headless(move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    assert_eq!(player.property("vo").unwrap(), "null");
    assert_eq!(player.property("ao").unwrap(), "null");
    player.load(clip.0.to_str().unwrap()).unwrap();
    event(&rx, |e| matches!(e, PlayerEvent::FileLoaded));
    assert!((number(&player, "duration") - 4.0).abs() < 0.05);
    let tracks = player.audio_tracks();
    assert_eq!(tracks.len(), 3);
    for (index, track) in tracks.iter().enumerate() {
        assert_eq!(index, track.index);
        player.select_audio_track(index).unwrap();
        until(|| player.property("aid").unwrap() == track.id.to_string());
    }
    assert!(player.select_audio_track(3).is_err());
    // MP4 stream titles are absent: selection must be by audio ordinal, never title matching.
    for i in 1..=3 {
        assert!(player.property(&format!("track-list/{i}/title")).is_err());
    }
    player.play().unwrap();
    event(&rx, |e| matches!(e, PlayerEvent::Position(v) if *v > 0.1));
    player.pause(true).unwrap();
    until(|| player.property("pause").as_deref() == Ok("yes"));
    // Drain old restart events before verifying the next request.
    for target in [2.35, 0.45, 3.1] {
        while rx.try_recv().is_ok() {}
        player.seek_exact(target).unwrap();
        event(&rx, |e| matches!(e, PlayerEvent::PlaybackRestart));
        assert!((number(&player, "time-pos") - target).abs() < 0.06);
    }
    while rx.try_recv().is_ok() {}
    player.seek_keyframe(2.65).unwrap();
    event(&rx, |e| matches!(e, PlayerEvent::PlaybackRestart));
    let keyframe = number(&player, "time-pos");
    assert!((2.0..=2.67).contains(&keyframe), "{keyframe}");
    let before = number(&player, "time-pos");
    player.frame_step().unwrap();
    until(|| number(&player, "time-pos") > before);
    assert_eq!(player.property("pause").unwrap(), "yes");
    player.set_volume(37.0).unwrap();
    player.set_mute(true).unwrap();
    player.set_speed(0.25).unwrap();
    assert_eq!(number(&player, "volume"), 37.0);
    assert_eq!(number(&player, "speed"), 0.25);
    assert_eq!(player.property("mute").unwrap(), "yes");
    player.set_mute(false).unwrap();
    assert_eq!(number(&player, "volume"), 37.0);
    player.set_speed(2.0).unwrap();
    player.seek_exact(3.5).unwrap();
    player.play().unwrap();
    event(&rx, |e| matches!(e, PlayerEvent::Eof(true)));
    assert_eq!(player.property("eof-reached").unwrap(), "yes");
    player.play().unwrap();
    until(|| number(&player, "time-pos") < 1.0);
    player.stop().unwrap();
    event(&rx, |e| matches!(e, PlayerEvent::Stopped));
    player.load(clip.0.to_str().unwrap()).unwrap();
    event(&rx, |e| matches!(e, PlayerEvent::FileLoaded));
    assert_eq!(number(&player, "speed"), 1.0);
    assert_eq!(player.property("loop-file").unwrap(), "no");
    drop(player);
}

#[test]
fn reject_invalid_native_inputs() {
    let player = MpvPlayer::headless(|_| {}).unwrap();
    assert!(player.seek_exact(f64::NAN).is_err());
    assert!(player.set_volume(f64::INFINITY).is_err());
    assert!(player.set_speed(0.0).is_err());
    assert!(player.command(&["loadfile", "bad\0path"]).is_err());
}
