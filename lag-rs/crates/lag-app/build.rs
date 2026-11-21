use std::{env, path::PathBuf, process::Command};

#[path = "build_twin.rs"]
mod build_twin;

fn main() {
    // The app root. ui/gallery.slint (design-system showcase) is type-checked by `cargo test -p lag-app`
    // via the gallery_compiles test instead of being compiled into the exe.
    // Lag's UI is loaded from ui/*.slint at run time (themes, docs/spec/themes.md); this generates its typed
    // API (the "twin") and type-checks every .slint file.
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    build_twin::generate(std::path::Path::new("ui/app.slint"), &out.join("ui_twin.rs"));
    println!("cargo:rerun-if-changed=build_twin.rs");
    println!("cargo:rerun-if-changed=ui");
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
    let plain = |p: &std::path::Path| p.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    let mut script = format!("1 ICON \"{}\"\n", plain(&ico));
    // 2 = light, 3 = GTA VI: the app points its shortcuts at these when the theme changes, so the taskbar button
    // follows the theme (src/shell/shortcut_icon.rs)
    for (id, name) in [(2, "icon-light.ico"), (3, "icon-gta.ico")] {
        let themed = manifest.join("assets").join(name);
        println!("cargo:rerun-if-changed={}", themed.display());
        match themed.canonicalize() {
            Ok(p) => script.push_str(&format!("{id} ICON \"{}\"\n", plain(&p))),
            Err(_) => println!("cargo:warning=theme icon not found: {}", themed.display()),
        }
    }
    if std::fs::write(&rc, script).is_err() {
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
