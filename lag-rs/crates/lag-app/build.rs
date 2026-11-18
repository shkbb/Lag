use std::{env, path::PathBuf, process::Command};

fn main() {
    // The app root. ui/gallery.slint (design-system showcase) is type-checked by `cargo test -p lag-app`
    // via the gallery_compiles test instead of being compiled into the exe.
    slint_build::compile("ui/app.slint").expect("slint compile app");
    embed_exe_icon();
}

/// Lag.exe's own icon (Explorer, shortcuts, the taskbar before the window sets its themed one) = C#'s
/// ApplicationIcon (Assets/icon.ico). GNU toolchain: windres compiles a one-line .rc into a COFF object that is
/// linked into the binary. No windres -> a warning, the build still succeeds (owner, 2026-11-17: "finally give
/// it an icon" - the taskbar showed a blank document).
fn embed_exe_icon() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("../../../InstantReplay/Assets/icon.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    let Ok(ico) = ico.canonicalize() else {
        println!("cargo:warning=app icon not found: {}", ico.display());
        return;
    };
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc = out.join("lag_icon.rc");
    let obj = out.join("lag_icon_res.o");
    // canonicalize() gives a verbatim path (\\?\E:\...): windres wants a plain one with forward slashes
    let ico_path = ico.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    if std::fs::write(&rc, format!("1 ICON \"{ico_path}\"\n")).is_err() {
        return;
    }
    for tool in ["windres", "x86_64-w64-mingw32-windres"] {
        let ok = Command::new(tool).arg(&rc).args(["-O", "coff", "-o"]).arg(&obj).status().is_ok_and(|s| s.success());
        if ok {
            println!("cargo:rustc-link-arg-bins={}", obj.display());
            return;
        }
    }
    println!("cargo:warning=windres not found: Lag.exe is built without its icon");
}
