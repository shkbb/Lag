//! Interface themes (docs/spec/themes.md, phase 2): a theme is a folder in `%APPDATA%\Lag\themes\<id>\` with a
//! `theme.toml` manifest, `.slint` files and pictures / fonts. Lag's own UI (`ui\`, shipped next to Lag.exe) is
//! the built-in theme "lag". A theme replaces the windows it defines (usually `AppWindow`); the rest stay Lag's.
//! It talks to Lag only through `@lag/api.slint` (the globals Lag fills). A theme that does not compile never
//! breaks Lag: the built-in UI is used and the error is kept for Settings.
//!
//! Sharing: one `.lagtheme` file = a zip of the theme folder. Packages are checked before use: only theme files
//! (`.slint`, pictures, fonts, text), no paths leaving the package, size limits. `.slint` is declarative: no code
//! runs, no file or network access beyond loading the package's own pictures and fonts.
use crate::ui_twin::{self, rt};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// The built-in theme's id.
pub const BUILTIN: &str = "lag";
/// The theme API version this Lag provides (`api` in theme.toml).
pub const API_VERSION: u32 = 1;
/// The API globals Lag's Rust code drives; re-exported for every theme.
const API_GLOBALS: &str =
    "Tr, MainVM, Page, WindowMetrics, SettingsVM, LibraryVM, PlayerVM, EditorVM, PlayerOverlayVM, EditorOverlayVM, AutoScroll";
const ALLOWED: &[&str] = &["slint", "toml", "md", "txt", "png", "jpg", "jpeg", "svg", "gif", "webp", "ttf", "otf"];
const MAX_FILE: u64 = 32 << 20;
const MAX_TOTAL: u64 = 96 << 20;
const MAX_FILES: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub api: u32,
    pub dir: PathBuf,
    pub entry: String,
    pub builtin: bool,
}

#[derive(Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    version: String,
    #[serde(default = "one")]
    api: u32,
    #[serde(default = "app")]
    entry: String,
}
fn one() -> u32 {
    1
}
fn app() -> String {
    "app.slint".into()
}

/// `%APPDATA%\Lag\themes`.
pub fn user_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Lag").join("themes"))
}

fn builtin() -> ThemeInfo {
    ThemeInfo {
        id: BUILTIN.into(),
        name: "Lag".into(),
        author: "Lag".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        api: API_VERSION,
        dir: ui_twin::builtin_dir(),
        entry: "app.slint".into(),
        builtin: true,
    }
}

/// Reads one theme folder.
pub fn read(dir: &Path) -> Result<ThemeInfo, String> {
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("no folder name")?;
    let text = std::fs::read_to_string(dir.join("theme.toml")).map_err(|e| format!("theme.toml: {e}"))?;
    let m: Manifest = toml::from_str(&text).map_err(|e| format!("theme.toml: {e}"))?;
    if m.name.trim().is_empty() {
        return Err("theme.toml: `name` is empty".into());
    }
    Ok(ThemeInfo { id, name: m.name, author: m.author, version: m.version, api: m.api, dir: dir.to_path_buf(), entry: m.entry, builtin: false })
}

/// The built-in theme first, then the installed ones by name.
pub fn list() -> Vec<ThemeInfo> {
    let mut themes = Vec::new();
    if let Some(Ok(entries)) = user_dir().map(std::fs::read_dir) {
        for e in entries.flatten().filter(|e| e.path().is_dir()) {
            match read(&e.path()) {
                Ok(t) if t.id != BUILTIN => themes.push(t),
                Ok(_) => {}
                Err(err) => log::warn!("[Theme] {}: {err}", e.path().display()),
            }
        }
    }
    themes.sort_by_key(|t| t.name.to_lowercase());
    themes.insert(0, builtin());
    themes
}

/// One path written inside a theme file (`import ... from "x"`, `@image-url("x")`, `import "font.ttf"`): either
/// Lag's API (`@lag/...`) or a file inside the package, never above it or elsewhere on disk.
fn path_ok(p: &str) -> bool {
    let rest = p.strip_prefix("@lag/").unwrap_or(p);
    if rest.is_empty() || p.contains(':') || p.starts_with('/') || p.starts_with('\\') || p.starts_with('@') && !p.starts_with("@lag/") {
        return false;
    }
    Path::new(rest).components().all(|c| matches!(c, Component::Normal(_)))
}

/// The paths a `.slint` text refers to.
fn referenced_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["from", "import", "@image-url("] {
        let mut at = 0;
        while let Some(i) = text[at..].find(key) {
            let start = at + i + key.len();
            at = start;
            let tail = text[start..].trim_start();
            if let Some(s) = tail.strip_prefix('"') {
                if let Some(end) = s.find('"') {
                    out.push(s[..end].to_owned());
                }
            }
        }
    }
    out
}

/// Checks a theme folder before it is loaded or installed.
pub fn validate(dir: &Path) -> Result<(), String> {
    let (mut files, mut total) = (0usize, 0u64);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
            let path = e.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            let rel = path.strip_prefix(dir).unwrap_or(&path).display().to_string();
            if meta.file_type().is_symlink() {
                return Err(format!("{rel}: links are not allowed in a theme"));
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            total += meta.len();
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !ALLOWED.contains(&ext.as_str()) {
                return Err(format!("{rel}: this kind of file cannot be in a theme"));
            }
            if meta.len() > MAX_FILE {
                return Err(format!("{rel}: too big ({} MB)", meta.len() >> 20));
            }
            if ext == "slint" {
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{rel}: {e}"))?;
                if let Some(bad) = referenced_paths(&text).into_iter().find(|p| !path_ok(p)) {
                    return Err(format!("{rel}: \"{bad}\" points outside the theme"));
                }
            }
        }
    }
    if files > MAX_FILES || total > MAX_TOTAL {
        return Err(format!("the theme is too big ({files} files, {} MB)", total >> 20));
    }
    Ok(())
}

thread_local! {
    /// (the theme the UI was built from, why the chosen one is not it)
    static ACTIVE: std::cell::RefCell<(String, Option<String>)> = std::cell::RefCell::new((BUILTIN.into(), None));
}

/// The theme the current UI was built from.
pub fn active() -> String {
    ACTIVE.with(|a| a.borrow().0.clone())
}

/// Why the chosen theme is not the active one (it failed to load), first line.
pub fn last_error() -> Option<String> {
    ACTIVE.with(|a| a.borrow().1.clone())
}

/// Loads Lag's own UI, then the theme `id` over it. Err only when Lag's own UI fails (Lag cannot start); a theme
/// that fails leaves Lag's own UI in place, logged and kept for Settings (`last_error`).
pub fn activate(id: &str) -> Result<Duration, String> {
    let started = Instant::now();
    ui_twin::load_builtin()?;
    let id = if id.is_empty() { BUILTIN } else { id };
    // Shift held while Lag starts: Lag's own interface for this run (a theme that hides everything)
    let shift = unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT.0 as i32)
    } < 0;
    if shift && id != BUILTIN {
        log::info!("[Theme] Shift held at start: Lag's own interface instead of \"{id}\"");
        ACTIVE.with(|a| *a.borrow_mut() = (BUILTIN.to_owned(), None));
        return Ok(started.elapsed());
    }
    let outcome = if id == BUILTIN { Ok(()) } else { apply(id) };
    ACTIVE.with(|a| {
        *a.borrow_mut() = match outcome {
            Ok(()) => (id.to_owned(), None),
            Err(e) => {
                log::warn!("[Theme] \"{id}\" did not load, Lag's own UI is used:\n{e}");
                (BUILTIN.to_owned(), Some(e.lines().next().unwrap_or("").to_owned()))
            }
        }
    });
    Ok(started.elapsed())
}

/// The theme `id` over the loaded built-in UI.
fn apply(id: &str) -> Result<(), String> {
    let theme = list().into_iter().find(|t| t.id == id).ok_or_else(|| format!("theme \"{id}\" is not installed"))?;
    if theme.api > API_VERSION {
        return Err(format!("\"{}\" needs a newer Lag (theme API {}, this Lag has {API_VERSION})", theme.name, theme.api));
    }
    validate(&theme.dir)?;
    let entry = theme.dir.join(&theme.entry);
    let api = ui_twin::builtin_dir();
    // first pass: which windows the theme draws
    let windows: Vec<String> = rt::compile(&entry, &api)?.iter().map(|c| c.name().to_owned()).collect();
    // the tray menu stays Lag's: it holds the way back to Lag's own interface
    let windows: Vec<String> = windows.into_iter().filter(|w| w != "TrayMenuWindow").collect();
    if windows.is_empty() {
        return Err(format!("\"{}\" exports no window (export component AppWindow inherits Window ...)", theme.name));
    }
    // second pass: the same windows plus the API globals, so Lag's Rust code reaches them
    let wrapper = std::env::temp_dir().join(format!("lag-theme-{}.slint", std::process::id()));
    let text = format!(
        "export {{ {} }} from \"{}\";\nexport {{ {API_GLOBALS} }} from \"@lag/api.slint\";\n",
        windows.join(", "),
        entry.display().to_string().replace('\\', "/")
    );
    std::fs::write(&wrapper, text).map_err(|e| e.to_string())?;
    let defs = rt::compile(&wrapper, &api);
    let _ = std::fs::remove_file(&wrapper);
    rt::install(defs?);
    log::info!("[Theme] \"{}\" {} by {} draws {}", theme.name, theme.version, theme.author, windows.join(", "));
    Ok(())
}

/// Installs a `.lagtheme` file into the themes folder (checked first; an installed theme of the same id is
/// replaced). Returns the theme.
pub fn import(file: &Path) -> Result<ThemeInfo, String> {
    let root = user_dir().ok_or("no %APPDATA%")?;
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let id: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if id.is_empty() || id == BUILTIN {
        return Err("bad theme file name".into());
    }
    let staging = root.join(format!(".import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = unpack(file, &staging).and_then(|_| {
        // a zip of the folder itself (one top folder with theme.toml) or of its contents
        let inner = match std::fs::read_dir(&staging).map_err(|e| e.to_string())?.flatten().collect::<Vec<_>>().as_slice() {
            [only] if only.path().is_dir() && only.path().join("theme.toml").is_file() => only.path(),
            _ => staging.clone(),
        };
        validate(&inner)?;
        read(&inner)?;
        let target = root.join(&id);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&inner, &target).map_err(|e| e.to_string())?;
        read(&target)
    });
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn unpack(file: &Path, into: &Path) -> Result<(), String> {
    let f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("not a theme file: {e}"))?;
    if zip.len() > MAX_FILES {
        return Err("the theme has too many files".into());
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name refuses absolute paths and ".." (zip-slip)
        let rel = entry.enclosed_name().ok_or_else(|| format!("unsafe path in the theme file: {}", entry.name()))?;
        let out = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        total += entry.size();
        if entry.size() > MAX_FILE || total > MAX_TOTAL {
            return Err("the theme is too big".into());
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        (&mut entry).take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes a theme folder as one `.lagtheme` file (Lag's own theme too: a starting point for a new one).
pub fn export(theme: &ThemeInfo, file: &Path) -> Result<(), String> {
    let f = std::fs::File::create(file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut stack = vec![theme.dir.clone()];
    let mut wrote_manifest = false;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !ALLOWED.contains(&ext.as_str()) {
                continue;
            }
            let rel = path.strip_prefix(&theme.dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            wrote_manifest |= rel == "theme.toml";
            zip.start_file(rel, opts).map_err(|e| e.to_string())?;
            zip.write_all(&std::fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
    }
    if !wrote_manifest {
        zip.start_file("theme.toml", opts).map_err(|e| e.to_string())?;
        let manifest = format!(
            "name = \"{} (copy)\"\nauthor = \"\"\nversion = \"1.0\"\napi = {API_VERSION}\nentry = \"{}\"\n",
            theme.name, theme.entry
        );
        zip.write_all(manifest.as_bytes()).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lag-theme-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn paths_in_a_theme_stay_inside_it() {
        for ok in ["@lag/api.slint", "@lag/components/buttons.slint", "parts/clock.slint", "art/bg.png", "font.ttf"] {
            assert!(path_ok(ok), "{ok}");
        }
        for bad in ["../x.slint", "C:/Windows/x.png", "/etc/x", "\\\\server\\x.png", "@lag/../../x", "parts/../../x", "@other/x"] {
            assert!(!path_ok(bad), "{bad}");
        }
        let text = "import { MainVM } from \"@lag/api.slint\";\nimport \"fonts/a.ttf\";\nImage { source: @image-url(\"../secret.png\"); }";
        assert_eq!(referenced_paths(text), vec!["@lag/api.slint", "fonts/a.ttf", "../secret.png"]);
    }

    #[test]
    fn packages_are_checked() {
        let d = temp("check");
        std::fs::write(d.join("theme.toml"), "name = \"T\"\n").unwrap();
        std::fs::write(d.join("app.slint"), "import { MainVM } from \"@lag/api.slint\";").unwrap();
        assert_eq!(validate(&d), Ok(()));
        std::fs::write(d.join("run.exe"), "MZ").unwrap();
        assert!(validate(&d).unwrap_err().contains("run.exe"));
        std::fs::remove_file(d.join("run.exe")).unwrap();
        std::fs::write(d.join("app.slint"), "Image { source: @image-url(\"C:/Users/x/Desktop/a.png\"); }").unwrap();
        assert!(validate(&d).unwrap_err().contains("outside the theme"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_then_import_round_trips_and_rejects_zip_slip() {
        let src = temp("src");
        std::fs::create_dir_all(src.join("art")).unwrap();
        std::fs::write(src.join("theme.toml"), "name = \"Rice\"\nauthor = \"me\"\nversion = \"2\"\n").unwrap();
        std::fs::write(src.join("app.slint"), "export component AppWindow inherits Window {}").unwrap();
        std::fs::write(src.join("art").join("bg.png"), [137u8, 80, 78, 71]).unwrap();
        let info = read(&src).unwrap();
        let file = std::env::temp_dir().join(format!("lag-rice-{}.lagtheme", std::process::id()));
        export(&info, &file).unwrap();
        let out = temp("out");
        unpack(&file, &out).unwrap();
        assert_eq!(std::fs::read(out.join("art").join("bg.png")).unwrap(), [137u8, 80, 78, 71]);
        assert_eq!(read(&out).unwrap().name, "Rice");

        // a crafted file that writes outside the folder
        let evil = std::env::temp_dir().join(format!("lag-evil-{}.lagtheme", std::process::id()));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&evil).unwrap());
        z.start_file("../../evil.slint", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
        let out2 = temp("out2");
        assert!(unpack(&evil, &out2).unwrap_err().contains("unsafe path"));
        for p in [src, out, out2] {
            let _ = std::fs::remove_dir_all(p);
        }
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_file(evil);
    }

    /// A theme compiled over Lag's own UI: its window replaces Lag's, the others stay, the API globals reach it.
    #[test]
    fn a_theme_replaces_only_its_windows() {
        let dir = temp("activate");
        std::fs::write(dir.join("theme.toml"), "name = \"Mini\"\n").unwrap();
        std::fs::write(
            dir.join("app.slint"),
            "import { MainVM } from \"@lag/api.slint\";\nexport component AppWindow inherits Window { Text { text: MainVM.status-text; } }",
        )
        .unwrap();
        let ui = ui_twin::builtin_dir();
        ui_twin::load_builtin().unwrap();
        let windows: Vec<String> = rt::compile(&dir.join("app.slint"), &ui).unwrap().iter().map(|c| c.name().to_owned()).collect();
        assert_eq!(windows, ["AppWindow"]);
        let wrapper = dir.join("wrapper.slint");
        std::fs::write(
            &wrapper,
            format!(
                "export {{ AppWindow }} from \"{}\";\nexport {{ {API_GLOBALS} }} from \"@lag/api.slint\";\n",
                dir.join("app.slint").display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();
        let defs = rt::compile(&wrapper, &ui).unwrap();
        // the API globals are visible through the theme's window
        let names: Vec<String> = defs[0].globals().collect();
        for g in ["MainVM", "SettingsVM", "Tr", "AutoScroll"] {
            assert!(names.iter().any(|n| n == g), "{g} in {names:?}");
        }
        rt::install(defs);
        let mut loaded = rt::loaded();
        loaded.sort();
        assert!(loaded.contains(&"ToastWindow".to_string()) && loaded.contains(&"AppWindow".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
