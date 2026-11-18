//! Headless parity checks for AacAudioEncoder.cs, MicDenoiser.cs and WASAPI capture services.
use lag_audio::{AacAudioEncoder, AudioTracks, MicDenoiser, PcmFormat, ProcessLoopbackCapture, ProcessLoopbackMode};
use lag_ffi::ffmpeg as ff;
use std::time::Duration;
fn sine(start: usize, frames: usize) -> Vec<u8> {
    (start..start + frames)
        .flat_map(|i| {
            let v = (std::f32::consts::TAU * 440. * i as f32 / 48000.).sin() * 0.35;
            [v.to_le_bytes(), v.to_le_bytes()].concat()
        })
        .collect()
}
#[test]
fn aac_sine_gap_padding_and_timeline() {
    let encoder = AacAudioEncoder::new(PcmFormat::SYSTEM, 160).unwrap();
    let mut packets = Vec::new();
    // 0..250ms sine, 250..500ms silence, 500..1000ms sine.
    for chunk in 0..25 {
        packets.extend(encoder.encode(&sine(chunk * 480, 480), chunk as i64 * 10000).unwrap());
    }
    for chunk in 50..100 {
        packets.extend(encoder.encode(&sine(chunk * 480, 480), chunk as i64 * 10000).unwrap());
    }
    packets.extend(encoder.pump_silence_to(1_000_000).unwrap());
    assert_eq!(encoder.written_samples(), 48000);
    packets.extend(encoder.flush().unwrap());
    assert_eq!(packets.len(), 47); // 46 complete frames + AAC priming; partial 896 samples stay in FIFO.
    assert_eq!(packets[0].pts_us, -21333);
    assert!(packets
        .windows(2)
        .all(|w| w[1].pts_us > w[0].pts_us && (21333..=21334).contains(&(w[1].pts_us - w[0].pts_us))));
    assert!(packets.iter().all(|p| p.is_keyframe && p.pts_us == p.dts_us && !p.data.is_empty()));
    let spec = encoder.get_stream_spec();
    assert_eq!((spec.sample_rate, spec.channels), (48000, 2));
    assert!(!spec.extra_data.is_empty());
    let decoded = decode(&packets, &spec.extra_data);
    assert_eq!(decoded.len(), packets.len() * 1024);
    let rms = |range: std::ops::Range<usize>| {
        (decoded[range.clone()].iter().map(|x| x * x).sum::<f32>() / range.len() as f32).sqrt()
    };
    // Decoder output includes one priming frame; avoid transition edges and transform ringing.
    assert!(rms(4000..10000) > 0.15);
    assert!(rms(17000..22000) < 0.005, "silence padding must encode silence");
    assert!(rms(28000..40000) > 0.15);
    assert!(encoder.flush().unwrap().is_empty());
}
#[test]
fn silence_pump_mixes_microphone_and_preserves_gap_limit() {
    let e = AacAudioEncoder::new(PcmFormat::SYSTEM, 160).unwrap();
    e.encode_mic(&sine(0, 4800), PcmFormat::SYSTEM).unwrap();
    let mut packets = e.pump_silence_to(100000).unwrap();
    assert_eq!(e.written_samples(), 4800);
    packets.extend(e.flush().unwrap());
    let decoded = decode(&packets, &e.get_stream_spec().extra_data);
    assert!(decoded.iter().any(|x| x.abs() > 0.1), "mic must survive silent system audio");
    let e = AacAudioEncoder::new(PcmFormat::SYSTEM, 160).unwrap();
    assert!(e.pump_silence_to(5_000_021).unwrap().is_empty());
    assert_eq!(e.written_samples(), 0);
    e.pump_silence_to(5_000_000).unwrap();
    assert_eq!(e.written_samples(), 240000);
}
#[test]
fn separate_tracks_are_ordered_and_microphone_is_lazy() {
    let tracks = AudioTracks::new(PcmFormat::SYSTEM, true).unwrap();
    let specs = tracks.specs();
    assert_eq!(specs.iter().map(|s| s.name).collect::<Vec<_>>(), ["All audio", "System audio", "Microphone"]);
    assert!(specs[2].stream.is_none());
    let packets = tracks.pump_silence_to(100000).unwrap();
    assert!(packets.iter().all(|p| p.track_index < 2));
    let mono = PcmFormat { channels: 1, ..PcmFormat::SYSTEM };
    tracks.encode_mic(&vec![0; 1920], mono, 100000).unwrap();
    assert!(tracks.specs()[2].stream.is_some());
    tracks.pump_silence_to(200000).unwrap();
    tracks.flush().unwrap();
    let single = AudioTracks::new(PcmFormat::SYSTEM, false).unwrap();
    assert_eq!(single.specs()[0].name, "Audio");
}

#[test]
fn unsupported_pcm_is_rejected_before_native_reads() {
    for format in [
        PcmFormat { bits_per_sample: 16, ..PcmFormat::SYSTEM },
        PcmFormat { bits_per_sample: 24, is_float: false, ..PcmFormat::SYSTEM },
        PcmFormat { channels: 0, ..PcmFormat::SYSTEM },
    ] {
        assert!(AacAudioEncoder::new(format, 160).is_err());
    }
}
#[test]
fn rnnoise_real_model_processes_one_second_noise() {
    let model = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../InstantReplay/rnnoise/std.rnnn");
    assert!(model.is_file(), "required bundled std.rnnn fixture missing: {}", model.display());
    let mut d = MicDenoiser::new(model);
    d.enabled = true;
    let mut seed = 42u32;
    let mut total = 0;
    for _ in 0..100 {
        let mut noise = Vec::with_capacity(3840);
        for _ in 0..960 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let v = (seed as f64 / u32::MAX as f64 - 0.5) as f32 * 0.05;
            noise.extend_from_slice(&v.to_le_bytes());
        }
        let out = d.process(&noise, 48000, 2).expect("real arnndn graph must build and process");
        total += out.len();
        assert!(out.as_chunks::<4>().0.iter().all(|b| f32::from_le_bytes(*b).is_finite()));
    }
    assert!((48000 * 8 - 9600..=48000 * 8 + 9600).contains(&total), "unexpected output byte count {total}");
    d.enabled = false;
    assert!(d.process(&[0; 80], 48000, 2).is_none());
}
#[test]
fn qpc_clock_freezes_and_resumes() {
    let c = lag_audio::clock::EngineClock::new();
    std::thread::sleep(Duration::from_millis(5));
    let before = c.engine_now_us();
    assert!(before > 0);
    c.set_paused(true);
    let paused = c.engine_now_us();
    std::thread::sleep(Duration::from_millis(20));
    assert!((c.engine_now_us() - paused).abs() <= 1);
    c.set_paused(false);
    assert!((c.engine_now_us() - paused).abs() < 5000);
    std::thread::sleep(Duration::from_millis(5));
    assert!(c.engine_now_us() > paused + 1000);
}

#[test]
fn audio_session_timer_pause_and_finish_without_real_capture() {
    use std::sync::{Arc, Mutex};
    let clock = Arc::new(lag_audio::clock::EngineClock::new());
    let packets = Arc::new(Mutex::new(Vec::new()));
    let out = packets.clone();
    let options = lag_audio::AudioSourceOptions {
        system_enabled: false,
        microphone_id: Some(String::new()),
        ..Default::default()
    };
    let mut session = lag_audio::tracks::AudioSession::start(
        options,
        Arc::new(lag_audio::AudioControls::default()),
        true,
        clock.clone(),
        move |p| {
            out.lock().unwrap().push(p);
        },
    )
    .unwrap();
    // Exercise the timer without opening either real endpoint.
    std::thread::sleep(Duration::from_millis(230));
    clock.set_paused(true);
    session.pump();
    let before = packets.lock().unwrap().len();
    assert!(before >= 12);
    std::thread::sleep(Duration::from_millis(120));
    session.pump();
    assert_eq!(packets.lock().unwrap().len(), before);
    clock.set_paused(false);
    std::thread::sleep(Duration::from_millis(30));
    session.pump();
    session.finish().unwrap();
    let count = packets.lock().unwrap().len();
    session.finish().unwrap();
    assert_eq!(packets.lock().unwrap().len(), count);
    let packets = packets.lock().unwrap();
    for index in 0..2 {
        let pts: Vec<_> = packets.iter().filter(|p| p.track_index == index).map(|p| p.packet.pts_us).collect();
        assert!(pts.len() >= 10);
        assert!(pts.windows(2).all(|w| w[1] > w[0]));
    }
    assert!(session.tracks().specs()[2].stream.is_none());
}
#[test]
fn enumerate_and_open_default_render_loopback_200ms() {
    let devices = lag_audio::devices::enumerate_devices().unwrap();
    assert!(devices.iter().any(|d| !d.is_capture));
    assert!(devices.iter().all(|d| !d.id.is_empty()));
    // Process bytes synchronously and discard; never persist or play real user audio.
    let options = lag_audio::AudioSourceOptions { microphone_id: Some(String::new()), ..Default::default() };
    let mut source = lag_audio::WasapiAudioSource::start(options, |_, _| {}, |_, _| {}).unwrap();
    assert!(source.game_format().sample_rate > 0);
    std::thread::sleep(Duration::from_millis(200));
    source.stop();
}
#[test]
fn process_loopback_opens_own_silent_tree_200ms() {
    let mut c = ProcessLoopbackCapture::start(std::process::id(), ProcessLoopbackMode::IncludeTree, |_| {}).unwrap();
    assert_eq!(c.format(), PcmFormat::SYSTEM);
    std::thread::sleep(Duration::from_millis(200));
    c.stop();
    let mut c = ProcessLoopbackCapture::start(std::process::id(), ProcessLoopbackMode::ExcludeTree, |_| {}).unwrap();
    c.stop();
}

#[test]
fn device_notifications_register_and_unregister_headlessly() {
    let watcher = lag_audio::devices::AudioDeviceWatcher::new(|| {}).unwrap();
    drop(watcher);
    let apps = lag_audio::sessions::get_active_audio_apps();
    let mut names = std::collections::HashSet::new();
    for app in apps {
        assert!(!app.exe_name.is_empty());
        assert!(!app.display_name.is_empty());
        assert!(names.insert(app.exe_name.to_lowercase()));
    }
}
#[test]
#[ignore = "explicit diagnostic: LAG_PROCTEST=<pid>, samples selected process for 5s"]
fn process_loopback_five_second_self_test() {
    ProcessLoopbackCapture::self_test(std::env::var("LAG_PROCTEST").unwrap().parse().unwrap()).unwrap();
}
#[test]
#[ignore = "explicit diagnostic: LAG_MFPROBE=file1;file2 (existing clips)"]
fn windows_audio_track_probe() {
    assert!(!std::env::var("LAG_MFPROBE").unwrap().is_empty());
    lag_audio::playback_probe::run_from_env();
}

#[test]
fn windows_probe_generated_separate_tracks_selects_all() {
    use std::os::windows::process::CommandExt;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("target/audio-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("tracks probe #{}.mp4", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(file.clone());
    let result = std::process::Command::new(root.join("../../../InstantReplay/ffmpeg/ffmpeg.exe"))
        .creation_flags(0x08000000) // CREATE_NO_WINDOW: headless fixture generation only.
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:r=10:d=1",
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=sample_rate=48000:duration=1:seed=42",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "1:a",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-b:a:0",
            "192k",
            "-b:a:1",
            "160k",
            "-b:a:2",
            "160k",
            "-metadata:s:a:0",
            "title=All",
            "-metadata:s:a:1",
            "title=System",
            "-metadata:s:a:2",
            "title=Microphone",
            "-disposition:a:0",
            "default",
            "-disposition:a:1",
            "0",
            "-disposition:a:2",
            "0",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(result.status.success(), "fixture generation: {}", String::from_utf8_lossy(&result.stderr));
    let report = lag_audio::playback_probe::probe(file.to_str().unwrap()).unwrap();
    assert_eq!(report.tracks.len(), 3);
    assert_eq!(report.selected_index, 0, "Windows must select the highest bitrate All mix: {report:?}");
    assert!(report.tracks[0].bitrate > report.tracks[1].bitrate);
    assert!(report.tracks[0].bitrate > report.tracks[2].bitrate);
}

fn decode(packets: &[lag_audio::EncodedPacket], extra: &[u8]) -> Vec<f32> {
    unsafe {
        let codec = ff::avcodec_find_decoder(ff::AV_CODEC_ID_AAC);
        assert!(!codec.is_null());
        let mut ctx = ff::avcodec_alloc_context3(codec);
        let mut frame = ff::av_frame_alloc();
        let mut pkt = ff::av_packet_alloc();
        (*ctx).extradata = ff::av_mallocz(extra.len() + 64).cast();
        (*ctx).extradata_size = extra.len() as i32;
        std::ptr::copy_nonoverlapping(extra.as_ptr(), (*ctx).extradata, extra.len());
        assert_eq!(ff::avcodec_open2(ctx, codec, std::ptr::null_mut()), 0);
        let mut samples = Vec::new();
        for p in packets {
            assert_eq!(ff::av_new_packet(pkt, p.data.len() as i32), 0);
            std::ptr::copy_nonoverlapping(p.data.as_ptr(), (*pkt).data, p.data.len());
            assert_eq!(ff::avcodec_send_packet(ctx, pkt), 0);
            ff::av_packet_unref(pkt);
            while ff::avcodec_receive_frame(ctx, frame) >= 0 {
                assert_eq!((*frame).format, ff::AV_SAMPLE_FMT_FLTP);
                samples.extend_from_slice(std::slice::from_raw_parts(
                    (*frame).data[0].cast::<f32>(),
                    (*frame).nb_samples as usize,
                ));
                ff::av_frame_unref(frame);
            }
        }
        ff::av_packet_free(&mut pkt);
        ff::av_frame_free(&mut frame);
        ff::avcodec_free_context(&mut ctx);
        samples
    }
}
