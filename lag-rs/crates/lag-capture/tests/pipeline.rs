//! Headless end-to-end check of the video path without screen capture:
//! synthetic BGRA frames -> converter -> hardware encoder -> PacketRing -> snapshot -> MP4 -> ffprobe.

use lag_capture::cadence::CadenceGrid;
use lag_capture::d3d::D3D11Context;
use lag_capture::encoder::{select, VideoEncoder};
use lag_capture::mux;
use lag_capture::ring::PacketRing;
use std::process::Command;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn ffprobe() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../InstantReplay/ffmpeg/ffprobe.exe")
}

#[test]
fn encode_ring_mux_probe() {
    let d3d = D3D11Context::new(-1).unwrap();
    let choice = select(None).expect("encoder");
    let mut enc = VideoEncoder::new(&d3d, choice, 1280, 720, 20_000, 4, 120).unwrap();
    let ring = PacketRing::new(5.0);
    let mut grid = CadenceGrid::new(120);

    // One reusable source texture, updated per frame (UpdateSubresource) like a capture pool would.
    let (w, h) = (1280u32, 720u32);
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex = None;
    unsafe { d3d.device.CreateTexture2D(&desc, None, Some(&mut tex)).unwrap() };
    let tex = tex.unwrap();
    let mut px = vec![0u8; (w * h * 4) as usize];

    // 8 s of jittery ~144 fps arrivals; the grid snaps them to 120 fps.
    let mut t_us: i64 = 10_000_000;
    let mut encoded = 0;
    for i in 0..(8 * 144) {
        t_us += 6_944 + ((i * 7919) % 2000) as i64 - 1000;
        let Some(pts) = grid.snap(t_us) else { continue };
        for (k, p) in px.chunks_exact_mut(4).enumerate() {
            p[0] = (k as i64 + i as i64 * 5) as u8;
            p[1] = (k / 1280) as u8;
            p[2] = (i * 2) as u8;
            p[3] = 255;
        }
        unsafe { d3d.context.UpdateSubresource(&tex, 0, None, px.as_ptr() as _, w * 4, 0) };
        for p in enc.encode(&tex, pts).unwrap() {
            ring.add(p);
        }
        encoded += 1;
    }
    for p in enc.flush() {
        ring.add(p);
    }
    let (clip, _base) = ring.snapshot(None);
    assert!(clip[0].is_keyframe);
    let span = (clip.last().unwrap().pts_us - clip[0].pts_us) as f64 / 1e6;
    eprintln!("encoded {encoded} frames, clip {} packets spanning {span:.2}s", clip.len());
    // 5 s window, start keyframe at/before the cutoff (keyframes every 2 s) -> 5..7 s.
    assert!((5.0..7.1).contains(&span), "span {span}");

    let out = std::env::temp_dir().join(format!("lag-rs-pipeline-{}.mp4", std::process::id()));
    let out_s = out.to_string_lossy().into_owned();
    assert!(mux::write(&out_s, &enc.stream_spec(), &clip, &[]));

    let probe = Command::new(ffprobe())
        .args(["-v", "error", "-count_frames", "-count_packets", "-select_streams", "v:0", "-show_entries",
            "stream=codec_name,width,height,nb_read_frames,nb_read_packets,nb_frames,color_range,color_space,color_primaries:format=duration",
            "-of", "default=nw=1"])
        .arg(&out)
        .output()
        .expect("run ffprobe");
    let text = String::from_utf8_lossy(&probe.stdout).to_string();
    eprintln!("{text}");
    if std::env::var_os("LAG_KEEP").is_none() {
        let _ = std::fs::remove_file(&out);
    } else {
        eprintln!("kept {}", out.display());
    }
    assert!(text.contains("codec_name=h264") || text.contains("codec_name=hevc"), "{text}");
    assert!(text.contains("width=1280") && text.contains("height=720"));
    assert!(text.contains("color_range=tv") && text.contains("color_space=bt709"), "{text}");
    assert!(text.contains(&format!("nb_read_frames={}", clip.len())), "every packet decodes: {text}");
}
