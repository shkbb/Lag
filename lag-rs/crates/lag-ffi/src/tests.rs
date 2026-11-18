use super::*;
use std::ffi::CString;

#[test]
fn ffmpeg_links_and_reports_7_1() {
    let v = ffmpeg_version();
    assert!(v.starts_with("n7.1"), "unexpected FFmpeg build {v}");
    let avcodec = unsafe { ffmpeg::avcodec_version() };
    assert_eq!(avcodec >> 16, 61, "avcodec major must be 61");
}

#[test]
fn encoders_used_by_the_engine_exist() {
    for name in ["h264_nvenc", "hevc_nvenc", "aac", "libx264"] {
        let c = CString::new(name).unwrap();
        let enc = unsafe { ffmpeg::avcodec_find_encoder_by_name(c.as_ptr()) };
        assert!(!enc.is_null(), "encoder {name} missing from the bundled FFmpeg");
    }
    // The FFmpeg 7.1.5 build shipped with Lag 1.7/1.8 has NO av1_nvenc (the cloud branch's rebuild adds it).
    // Same as the C# app: AV1 is optional and only offered when the encoder actually opens.
    let av1 = CString::new("av1_nvenc").unwrap();
    let has_av1 = unsafe { !ffmpeg::avcodec_find_encoder_by_name(av1.as_ptr()).is_null() };
    eprintln!("av1_nvenc present: {has_av1}");
}

#[test]
fn error_strings() {
    assert_eq!(AVERROR_EAGAIN, -11);
    assert!(!av_err_string(AVERROR_EAGAIN).is_empty());
}

#[test]
fn mpv_initialises_headless() {
    unsafe {
        let api = mpv::mpv_client_api_version();
        assert!(api >> 16 >= 2, "libmpv client API {api:#x}");
        let h = mpv::mpv_create();
        assert!(!h.is_null());
        for (k, v) in [("vo", "null"), ("ao", "null"), ("idle", "yes")] {
            let (k, v) = (CString::new(k).unwrap(), CString::new(v).unwrap());
            assert_eq!(mpv::mpv_set_option_string(h, k.as_ptr(), v.as_ptr()), 0);
        }
        let t = std::time::Instant::now();
        assert_eq!(mpv::mpv_initialize(h), 0);
        eprintln!("mpv_initialize took {:?}", t.elapsed());
        mpv::mpv_terminate_destroy(h);
    }
}
