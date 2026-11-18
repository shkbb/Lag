// Links lag-ffi against the same FFmpeg 7.1 DLLs the C# app ships (InstantReplay/ffmpeg) and libmpv.
// GNU ld links straight against a .dll, so no import libraries are needed for FFmpeg.
use std::{env, path::PathBuf};

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ffmpeg =
        env::var("LAG_FFMPEG_DIR").map(PathBuf::from).unwrap_or_else(|_| dir.join("../../../InstantReplay/ffmpeg"));
    let mpv = env::var("LAG_MPV_DIR").map(PathBuf::from).unwrap_or_else(|_| dir.join("../../vendor/mpv"));
    println!("cargo:rustc-link-search=native={}", ffmpeg.display());
    println!("cargo:rustc-link-search=native={}", mpv.display());
    for lib in ["avcodec-61", "avformat-61", "avutil-59", "avfilter-10", "swresample-5", "swscale-8"] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
    println!("cargo:rustc-link-lib=dylib=mpv");

    // Put the runtime DLLs next to every binary/test cargo produces (target/<profile> and its deps/),
    // so tests and examples run without PATH tweaks.
    if let Ok(out) = env::var("OUT_DIR") {
        let profile_dir = PathBuf::from(out).join("../../..");
        let dlls = std::fs::read_dir(&ffmpeg)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .chain(std::iter::once(mpv.join("libmpv-2.dll")))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")));
        for dll in dlls {
            for dir in [profile_dir.clone(), profile_dir.join("deps"), profile_dir.join("examples")] {
                let _ = std::fs::create_dir_all(&dir);
                let dst = dir.join(dll.file_name().unwrap());
                let stale = match (std::fs::metadata(&dll), std::fs::metadata(&dst)) {
                    (Ok(s), Ok(d)) => s.len() != d.len(),
                    _ => true,
                };
                if stale {
                    let _ = std::fs::copy(&dll, &dst);
                }
            }
        }
        // The runtime folders the app opens next to Lag.exe: ffmpeg/ (ffmpeg.exe + ffprobe.exe for the editor
        // export, thumbnails and the carry-over re-encode) and rnnoise/std.rnnn (mic noise suppression). Without
        // them a plain `cargo build` folder silently lost those features (settings audit 2026-11-17: noise
        // suppression switched itself off in target/release). tools/package.ps1 checks the same list.
        let root = dir.join("../../../InstantReplay");
        for sub in ["ffmpeg", "rnnoise"] {
            let src = env::var(format!("LAG_{}_DIR", sub.to_uppercase()))
                .map(PathBuf::from)
                .unwrap_or_else(|_| root.join(sub));
            let dst_dir = profile_dir.join(sub);
            let _ = std::fs::create_dir_all(&dst_dir);
            for f in std::fs::read_dir(&src).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())) {
                if !f.is_file() {
                    continue;
                }
                let dst = dst_dir.join(f.file_name().unwrap());
                let stale = match (std::fs::metadata(&f), std::fs::metadata(&dst)) {
                    (Ok(s), Ok(d)) => s.len() != d.len(),
                    _ => true,
                };
                if stale {
                    let _ = std::fs::copy(&f, &dst);
                }
            }
            println!("cargo:rerun-if-changed={}", src.display());
        }
    }
    println!("cargo:rerun-if-env-changed=LAG_FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=LAG_MPV_DIR");
}
