//! Interface themes (docs/spec/themes.md, phase 2): a theme is a folder in `%APPDATA%\Lag\themes\<id>\` with a
//! `theme.toml` manifest, `.slint` files and pictures / fonts. Lag's own UI (`ui\`, shipped next to Lag.exe) is
//! the built-in theme "lag". A theme replaces the windows it defines (usually `AppWindow`); the rest stay Lag's.
//! It talks to Lag only through `@lag/api.slint` (the globals Lag fills). A theme that does not compile never
//! breaks Lag: the built-in UI is used and the error is kept for Settings.
//!
//! Inheritance: a theme holds only the files it changes. Before it is compiled, Lag's own UI and the theme's
//! files are put together in a build folder (`sync`: Lag's file wherever the theme has none, the API always
//! Lag's), and `@lag/` points there. A newer Lag brings its new pages and settings into every theme that did
//! not change those files.
//!
//! One file: a theme is `<themes>\<id>.lag` (crates/lag-theme-file: UTF-8 text, the same on Windows and Linux),
//! shared as it is. Before Lag uses it, it is written out into a working folder of this process (`materialize`:
//! `%TEMP%\lag-theme-<pid>\own-...`, `ThemeInfo::dir`); the editor changes that folder and writes it back into the
//! one file after every change (`pack`). Themes made as folders before 2.2 (`<themes>\<id>\theme.toml`) and
//! `.lagtheme` zips are still read; `convert` turns a folder into one file. Packages are checked before use: only
//! theme files (`.slint`, pictures, fonts, video, text), no paths leaving the theme, size limits. `.slint` is
//! declarative: no code runs, no file or network access beyond loading the theme's own pictures and fonts.
use crate::ui_twin::{self, rt};
use lag_core::i18n::{format as trf, tr};
use serde::Deserialize;
use lag_theme_file::ThemeFile;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// The theme API reference for authors (generated from ui/api at build time; docs/themes/API.md).
pub const API_REFERENCE: &str = include_str!(concat!(env!("OUT_DIR"), "/theme_api.md"));

/// Puts `API.md` into the themes folder (next to the authors' themes).
pub fn write_reference(root: &Path) {
    let _ = std::fs::create_dir_all(root);
    let _ = std::fs::write(root.join("API.md"), API_REFERENCE);
}

/// The built-in theme's id.
pub const BUILTIN: &str = "lag";
/// The theme API version this Lag provides (`api` in theme.toml).
pub const API_VERSION: u32 = 1;
/// The API globals Lag's Rust code drives; re-exported for every theme.
const API_GLOBALS: &str =
    "Tr, MainVM, Page, WindowMetrics, SettingsVM, LibraryVM, PlayerVM, EditorVM, PlayerOverlayVM, EditorOverlayVM, AutoScroll, Media";
const ALLOWED: &[&str] =
    &["slint", "toml", "md", "txt", "png", "jpg", "jpeg", "svg", "gif", "webp", "ttf", "otf", "mp4", "webm"];
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
    /// where the theme's files are: the folder of a folder theme, the working folder of a `.lag` one
    pub dir: PathBuf,
    pub entry: String,
    pub builtin: bool,
    /// the `.lag` file (None: a folder theme, or Lag's own)
    pub file: Option<PathBuf>,
}

impl ThemeInfo {
    /// What the user sees and shares: the `.lag` file, or the folder.
    pub fn location(&self) -> &Path {
        self.file.as_deref().unwrap_or(&self.dir)
    }
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
        file: None,
    }
}

fn is_lag(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(lag_theme_file::EXTENSION)) && path.is_file()
}

/// The working folder of a `.lag` theme in this process.
fn own_dir(file: &Path, id: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    let id: String = id.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let dir = std::env::temp_dir().join(format!("lag-theme-{}", std::process::id())).join(format!("own-{id}-{:08x}", h.finish() as u32));
    i_slint_compiler::pathutils::clean_path(&dir)
}

fn file_error(e: lag_theme_file::Error) -> String {
    use lag_theme_file::Error as E;
    match e {
        E::NotATheme => trf("Theme_ErrNotTheme", &["#@lag-theme 1"]).unwrap_or_default(),
        E::Header(h) => format!("{}: {h}", tr("Theme_ErrNoName")),
        E::BadPath(p) => trf("Theme_ErrOutside", &[&p, &p]).unwrap_or_default(),
        E::Kind(p) => trf("Theme_ErrFileKind", &[&p]).unwrap_or_default(),
        E::TooBig(Some(p)) => trf("Theme_ErrFileTooBig", &[&p, "64"]).unwrap_or_default(),
        E::TooBig(None) => tr("Theme_ErrTooBig").to_owned(),
        E::TooMany => tr("Theme_ErrTooMany").to_owned(),
        other => other.to_string(),
    }
}

/// Reads a theme: a `.lag` file (its header only) or a theme folder.
pub fn read(path: &Path) -> Result<ThemeInfo, String> {
    if is_lag(path) {
        let id = path.file_stem().map(|n| n.to_string_lossy().into_owned()).ok_or("no file name")?;
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let m = ThemeFile::read_manifest(&text).map_err(file_error)?;
        return Ok(ThemeInfo {
            dir: own_dir(path, &id),
            id,
            name: m.name,
            author: m.author,
            version: m.version,
            api: m.api,
            entry: m.entry,
            builtin: false,
            file: Some(path.to_path_buf()),
        });
    }
    let dir = path;
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("no folder name")?;
    let text = std::fs::read_to_string(dir.join("theme.toml")).map_err(|e| format!("theme.toml: {e}"))?;
    let m: Manifest = toml::from_str(&text).map_err(|e| format!("theme.toml: {e}"))?;
    if m.name.trim().is_empty() {
        return Err(tr("Theme_ErrNoName").to_owned());
    }
    Ok(ThemeInfo {
        id,
        name: m.name,
        author: m.author,
        version: m.version,
        api: m.api,
        dir: dir.to_path_buf(),
        entry: m.entry,
        builtin: false,
        file: None,
    })
}

/// Writes a `.lag` theme out into its working folder (fresh: what the file says now). A folder theme: nothing to do.
pub fn materialize(theme: &ThemeInfo) -> Result<(), String> {
    let Some(file) = &theme.file else { return Ok(()) };
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let tf = ThemeFile::parse(&text).map_err(file_error)?;
    let _ = std::fs::remove_dir_all(&theme.dir);
    std::fs::create_dir_all(&theme.dir).map_err(|e| e.to_string())?;
    for (rel, content) in &tf.files {
        let out = rel.split('/').fold(theme.dir.clone(), |p, part| p.join(part));
        std::fs::create_dir_all(out.parent().unwrap_or(&theme.dir)).map_err(|e| e.to_string())?;
        std::fs::write(&out, content.bytes()).map_err(|e| format!("{rel}: {e}"))?;
    }
    write_manifest(&theme.dir, &tf.manifest)
}

fn write_manifest(dir: &Path, m: &lag_theme_file::Manifest) -> Result<(), String> {
    let mut table = toml::Table::new();
    table.insert("name".into(), toml::Value::String(m.name.clone()));
    table.insert("author".into(), toml::Value::String(m.author.clone()));
    table.insert("version".into(), toml::Value::String(m.version.clone()));
    table.insert("api".into(), toml::Value::Integer(m.api as i64));
    table.insert("entry".into(), toml::Value::String(m.entry.clone()));
    std::fs::write(dir.join("theme.toml"), toml::to_string(&table).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// A theme folder (a working folder, or one made before 2.2) as one `.lag` theme.
fn folder_to_file(dir: &Path) -> Result<ThemeFile, String> {
    let info = read(dir)?;
    let mut tf = ThemeFile::new(&info.name);
    tf.manifest.author = info.author;
    tf.manifest.version = if info.version.is_empty() { "1.0".into() } else { info.version };
    tf.manifest.api = info.api;
    tf.manifest.entry = info.entry;
    for (rel, path) in ui_files(dir) {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        tf.put(&rel.join("/"), bytes).map_err(file_error)?;
    }
    Ok(tf)
}

/// Writes `tf` into `file` safely (a temporary file renamed over it: a crash never leaves half a theme).
fn write_file(file: &Path, tf: &ThemeFile) -> Result<(), String> {
    let tmp = file.with_extension("lag.tmp");
    std::fs::write(&tmp, tf.to_text()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Writes a `.lag` theme's working folder back into its one file (the editor, after every change).
pub fn pack(theme: &ThemeInfo) -> Result<(), String> {
    let Some(file) = &theme.file else { return Ok(()) };
    write_file(file, &folder_to_file(&theme.dir)?)
}

/// The built-in theme first, then the installed ones by name.
pub fn list() -> Vec<ThemeInfo> {
    let mut themes = Vec::new();
    if let Some(Ok(entries)) = user_dir().map(std::fs::read_dir) {
        for e in entries.flatten().filter(|e| e.path().is_dir() || is_lag(&e.path())) {
            // a folder of something else (no theme.toml): not a theme
            if e.path().is_dir() && !e.path().join("theme.toml").is_file() {
                continue;
            }
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

/// Where a path written in a theme file leads, as package-relative parts (`file_dir` = the file's folder inside
/// the package). None = outside the package (above its root, a drive, an absolute path, another library).
fn resolve(file_dir: &[String], p: &str) -> Option<Vec<String>> {
    if p.is_empty() || p.contains(':') || p.starts_with('/') || p.starts_with('\\') || p.starts_with('@') {
        return None;
    }
    let mut at: Vec<String> = file_dir.to_vec();
    for c in Path::new(p).components() {
        match c {
            Component::Normal(n) => at.push(n.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                at.pop()?;
            }
            _ => return None,
        }
    }
    Some(at)
}

/// One path written inside a theme file (`import ... from "x"`, `@image-url("x")`, `import "font.ttf"`): either
/// Lag's API (`@lag/...`, no way up out of it) or a file inside the package, never above it or elsewhere on disk.
fn path_ok(file_dir: &[String], p: &str) -> bool {
    match p.strip_prefix("@lag/") {
        Some(rest) => !rest.is_empty() && resolve(&[], rest).is_some(),
        None => resolve(file_dir, p).is_some(),
    }
}

/// The text without `//` and `/* */` comments (words in comments are not imports).
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut rest, mut in_string) = (text, false);
    while let Some(c) = rest.chars().next() {
        if !in_string && rest.starts_with("//") {
            rest = &rest[rest.find('\n').unwrap_or(rest.len())..];
        } else if !in_string && rest.starts_with("/*") {
            rest = rest.get(2..).map_or("", |r| r.find("*/").map_or("", |e| &r[e + 2..]));
        } else {
            in_string ^= c == '"';
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The paths a `.slint` text refers to, with their byte ranges in the comment-free text.
fn path_refs(text: &str) -> Vec<(usize, usize)> {
    let word = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for key in ["from", "import", "@image-url("] {
        let mut at = 0;
        while let Some(i) = text[at..].find(key) {
            let (s, e) = (at + i, at + i + key.len());
            at = e;
            if key != "@image-url(" && (word(s.checked_sub(1).map(|j| bytes[j])) || word(bytes.get(e).copied())) {
                continue;
            }
            let tail = &text[e..];
            let skipped = tail.len() - tail.trim_start().len();
            if tail.trim_start().starts_with('"') {
                let start = e + skipped + 1;
                if let Some(len) = text[start..].find('"') {
                    out.push((start, start + len));
                }
            }
        }
    }
    out
}

fn referenced_paths(text: &str) -> Vec<String> {
    let clean = strip_comments(text);
    path_refs(&clean).into_iter().map(|(s, e)| clean[s..e].to_owned()).collect()
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
                return Err(trf("Theme_ErrLink", &[&rel]).unwrap_or_default());
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            total += meta.len();
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !ALLOWED.contains(&ext.as_str()) {
                return Err(trf("Theme_ErrFileKind", &[&rel]).unwrap_or_default());
            }
            if meta.len() > MAX_FILE {
                return Err(trf("Theme_ErrFileTooBig", &[&rel, &(meta.len() >> 20).to_string()]).unwrap_or_default());
            }
            if ext == "slint" {
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{rel}: {e}"))?;
                let file_dir: Vec<String> = path
                    .parent()
                    .and_then(|p| p.strip_prefix(dir).ok())
                    .map(|p| p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect())
                    .unwrap_or_default();
                if let Some(bad) = referenced_paths(&text).into_iter().find(|p| !path_ok(&file_dir, p)) {
                    return Err(trf("Theme_ErrOutside", &[&rel, &bad]).unwrap_or_default());
                }
            }
        }
    }
    if files > MAX_FILES || total > MAX_TOTAL {
        log::info!("[Theme] too big: {files} files, {} MB", total >> 20);
        return Err(tr("Theme_ErrTooBig").to_owned());
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
    let theme = list().into_iter().find(|t| t.id == id).ok_or_else(|| trf("Theme_ErrNotInstalled", &[id]).unwrap_or_default())?;
    apply_theme(&theme)
}

/// A theme from any folder (the preview, a theme being written) over the loaded built-in UI.
pub fn apply_dir(dir: &Path) -> Result<(), String> {
    apply_theme(&read(dir)?)
}

fn apply_theme(theme: &ThemeInfo) -> Result<(), String> {
    // what the file says now (the preview reloads after every save of it)
    materialize(theme)?;
    let (defs, windows) = compile_theme(theme)?;
    rt::install(defs);
    log::info!("[Theme] \"{}\" {} by {} draws {}", theme.name, theme.version, theme.author, windows.join(", "));
    Ok(())
}

/// A path of Lag's UI that a theme cannot replace: the API (the globals Lag's Rust code drives; a theme's own copy
/// would be globals nobody fills).
fn is_api(rel: &[String]) -> bool {
    rel.first().is_some_and(|f| f == "api" || f == "api.slint")
}

/// Every file of a UI folder that can be part of the interface, by its relative path.
fn ui_files(dir: &Path) -> Vec<(Vec<String>, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), Vec::<String>::new())];
    while let Some((d, rel)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let mut here = rel.clone();
            here.push(e.file_name().to_string_lossy().into_owned());
            if path.is_dir() {
                stack.push((path, here));
                continue;
            }
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ALLOWED.contains(&ext.as_str()) && ext != "toml" {
                out.push((here, path));
            }
        }
    }
    out
}

/// Where a theme is put together before it is compiled: `%TEMP%\lag-theme-<process>\<theme>` (one folder per
/// process: Lag, its preview and the editor may build the same theme at once).
pub fn build_dir(theme: &ThemeInfo) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    theme.dir.hash(&mut h);
    let id: String = theme.id.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let dir = std::env::temp_dir().join(format!("lag-theme-{}", std::process::id())).join(format!("{id}-{:08x}", h.finish() as u32));
    i_slint_compiler::pathutils::clean_path(&dir)
}

/// The build folders of processes that are gone.
fn remove_stale_builds() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("lag-theme-").and_then(|p| p.parse::<u32>().ok()) else { continue };
        if pid == std::process::id() {
            continue;
        }
        let alive = unsafe {
            use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let _ = windows::Win32::Foundation::CloseHandle(h);
                    true
                }
                Err(_) => false,
            }
        };
        if !alive {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Puts the theme together over Lag's own UI in its build folder (only what changed is copied) and returns it.
pub fn sync(theme: &ThemeInfo) -> Result<PathBuf, String> {
    static CLEANED: std::sync::Once = std::sync::Once::new();
    CLEANED.call_once(remove_stale_builds);
    let build = build_dir(theme);
    let mut wanted: std::collections::HashMap<Vec<String>, PathBuf> = ui_files(&ui_twin::builtin_dir()).into_iter().collect();
    if !theme.builtin {
        for (rel, path) in ui_files(&theme.dir) {
            if !is_api(&rel) {
                wanted.insert(rel, path);
            }
        }
    }
    let stamp = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    for (rel, src) in &wanted {
        let dest = rel.iter().fold(build.clone(), |p, part| p.join(part));
        if stamp(&dest).is_some() && stamp(&dest) == stamp(src) {
            continue;
        }
        std::fs::create_dir_all(dest.parent().unwrap_or(&build)).map_err(|e| e.to_string())?;
        std::fs::copy(src, &dest).map_err(|e| format!("{}: {e}", rel.join("/")))?;
        // the same time as the source: the next sync sees it is current
        if let Some(t) = std::fs::metadata(src).ok().and_then(|m| m.modified().ok()) {
            let _ = std::fs::File::options().write(true).open(&dest).and_then(|f| f.set_modified(t));
        }
    }
    // a file the theme no longer has (or Lag no longer ships)
    for (rel, path) in ui_files(&build) {
        if !wanted.contains_key(&rel) {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(build)
}

/// Compiles a theme the way Lag runs it: its windows (not the tray menu) plus the API globals. Returns the window
/// definitions and their names.
pub fn compile_theme(theme: &ThemeInfo) -> Result<(Vec<slint_interpreter::ComponentDefinition>, Vec<String>), String> {
    if theme.api > API_VERSION {
        return Err(trf("Theme_ErrNewer", &[&theme.name]).unwrap_or_default());
    }
    if theme.file.is_some() && !theme.dir.join("theme.toml").is_file() {
        materialize(theme)?;
    }
    validate(&theme.dir)?;
    // the theme over Lag's own UI; `@lag/` leads there too, so the theme's files and Lag's share one API
    let api = sync(theme)?;
    let entry = api.join(&theme.entry);
    // the compiler names the build folder: the author knows the theme's own paths
    let prefix = format!("{}{}", api.display(), std::path::MAIN_SEPARATOR);
    let tidy = |e: String| e.replace(&prefix, "");
    // first pass: which windows the theme draws
    let windows: Vec<String> = rt::compile(&entry, &api).map_err(tidy)?.iter().map(|c| c.name().to_owned()).collect();
    // the tray menu stays Lag's: it holds the way back to Lag's own interface
    let windows: Vec<String> = windows.into_iter().filter(|w| w != "TrayMenuWindow").collect();
    if windows.is_empty() {
        return Err(trf("Theme_ErrNoWindow", &[&theme.name]).unwrap_or_default());
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
    Ok((defs.map_err(tidy)?, windows))
}

fn unique_id(root: &Path, name: &str) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    let base = if base.is_empty() || base == BUILTIN { "theme".to_owned() } else { base };
    let free = |id: &String| !root.join(id).exists() && !root.join(format!("{id}.{}", lag_theme_file::EXTENSION)).exists();
    (1..).map(|i| if i == 1 { base.clone() } else { format!("{base}-{i}") }).find(free).unwrap_or(base)
}

/// A free theme name: `name`, or `name 2`, `name 3`... (the list tells them apart).
pub fn free_name(root: &Path, name: &str) -> String {
    let taken: Vec<String> = std::fs::read_dir(root)
        .map(|e| e.flatten().filter_map(|e| read(&e.path()).ok()).map(|t| t.name.to_lowercase()).collect())
        .unwrap_or_default();
    (1..)
        .map(|i| if i == 1 { name.to_owned() } else { format!("{name} {i}") })
        .find(|n| !taken.contains(&n.to_lowercase()))
        .unwrap_or_else(|| name.to_owned())
}

/// "New theme": `<root>\<id>.lag`, a theme named `name` that is Lag's own interface until something is changed in
/// it (it holds only what is changed; everything else comes from the Lag that runs it). Returns it.
pub fn new_from_builtin(root: &Path, name: &str) -> Result<ThemeInfo, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(tr("Theme_ErrNoName").to_owned());
    }
    let name = free_name(root, name);
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let file = root.join(format!("{}.{}", unique_id(root, &name), lag_theme_file::EXTENSION));
    write_file(&file, &ThemeFile::new(&name))?;
    read(&file)
}

/// Renames a theme (its name inside it; the file / folder stays). Returns the theme.
pub fn rename(theme: &ThemeInfo, name: &str) -> Result<ThemeInfo, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(tr("Theme_ErrNoName").to_owned());
    }
    // the folder's manifest: a folder theme's own, or the working folder the editor writes back from
    let manifest = theme.dir.join("theme.toml");
    if manifest.is_file() {
        let text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        let mut table: toml::Table = toml::from_str(&text).map_err(|e| format!("theme.toml: {e}"))?;
        table.insert("name".into(), toml::Value::String(name.to_owned()));
        std::fs::write(&manifest, toml::to_string(&table).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    if let Some(file) = &theme.file {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let mut tf = ThemeFile::parse(&text).map_err(file_error)?;
        tf.manifest.name = name.to_owned();
        write_file(file, &tf)?;
    }
    let mut t = read(theme.location())?;
    t.dir = theme.dir.clone();
    Ok(t)
}

/// A theme folder made before 2.2 as one `.lag` file next to it; the folder goes to the Recycle Bin.
pub fn convert(theme: &ThemeInfo) -> Result<ThemeInfo, String> {
    if theme.builtin || theme.file.is_some() {
        return Ok(theme.clone());
    }
    let root = theme.dir.parent().ok_or("no themes folder")?;
    let tf = folder_to_file(&theme.dir)?;
    let file = root.join(format!("{}.{}", theme.id, lag_theme_file::EXTENSION));
    if file.exists() {
        return Err(format!("{} already exists", file.display()));
    }
    write_file(&file, &tf)?;
    to_recycle_bin(&theme.dir)?;
    read(&file)
}

/// Moves an installed theme (its `.lag` file or its folder) to the Recycle Bin (it can be restored from there).
pub fn remove(theme: &ThemeInfo) -> Result<(), String> {
    if theme.builtin {
        return Err(tr("Theme_ErrDeleteBuiltin").to_owned());
    }
    to_recycle_bin(theme.location())
}

/// The Recycle Bin on Windows (a Linux build uses the freedesktop trash: docs/spec/themes.md, parity).
fn to_recycle_bin(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    // a double-NUL-terminated list of one path
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 || op.fAnyOperationsAborted.as_bool() || path.exists() {
        return Err(format!("{} ({r})", path.display()));
    }
    Ok(())
}

/// Installs a theme file into the themes folder as one `.lag`: a `.lag` (checked first), or a `.lagtheme` zip made
/// before 2.2 (converted). Never over an installed theme: the same file twice is two themes (`name-2`). Returns it.
pub fn import(file: &Path) -> Result<ThemeInfo, String> {
    let root = user_dir().ok_or("no %APPDATA%")?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let id: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if id.trim_matches('-').is_empty() || id == BUILTIN {
        return Err(tr("Theme_ErrFileName").to_owned());
    }
    let tf = if is_lag(file) {
        let text = std::fs::read_to_string(file).map_err(|_| trf("Theme_ErrNotTheme", &["UTF-8"]).unwrap_or_default())?;
        ThemeFile::parse(&text).map_err(file_error)?
    } else {
        let staging = std::env::temp_dir().join(format!("lag-theme-{}", std::process::id())).join("import");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let result = unpack(file, &staging).and_then(|_| {
            // a zip of the folder itself (one top folder with theme.toml) or of its contents
            let inner = match std::fs::read_dir(&staging).map_err(|e| e.to_string())?.flatten().collect::<Vec<_>>().as_slice() {
                [only] if only.path().is_dir() && only.path().join("theme.toml").is_file() => only.path(),
                _ => staging.clone(),
            };
            validate(&inner)?;
            folder_to_file(&inner)
        });
        let _ = std::fs::remove_dir_all(&staging);
        result?
    };
    // the references inside it are checked as for any theme (written out, then validated)
    let target = root.join(format!("{}.{}", unique_id(&root, &id), lag_theme_file::EXTENSION));
    write_file(&target, &tf)?;
    let theme = read(&target)?;
    let checked = materialize(&theme).and_then(|_| validate(&theme.dir));
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&target);
        return Err(e);
    }
    Ok(theme)
}

fn unpack(file: &Path, into: &Path) -> Result<(), String> {
    let f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| trf("Theme_ErrNotTheme", &[&e.to_string()]).unwrap_or_default())?;
    if zip.len() > MAX_FILES {
        return Err(tr("Theme_ErrTooMany").to_owned());
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name refuses absolute paths and ".." (zip-slip)
        let rel = entry.enclosed_name().ok_or_else(|| trf("Theme_ErrUnsafe", &[entry.name()]).unwrap_or_default())?;
        let out = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        total += entry.size();
        if entry.size() > MAX_FILE || total > MAX_TOTAL {
            return Err(tr("Theme_ErrTooBig").to_owned());
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

/// Writes a theme as one `.lag` file to share (Lag's own: an empty theme, a starting point).
pub fn export(theme: &ThemeInfo, file: &Path) -> Result<(), String> {
    let tf = if theme.builtin {
        ThemeFile::new(&format!("{} ({})", theme.name, tr("Theme_Copy")))
    } else if let Some(src) = &theme.file {
        let text = std::fs::read_to_string(src).map_err(|e| e.to_string())?;
        ThemeFile::parse(&text).map_err(file_error)?
    } else {
        folder_to_file(&theme.dir)?
    };
    write_file(file, &tf)
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
        let root: Vec<String> = vec![];
        let sub = vec!["views".to_string()];
        for ok in ["@lag/api.slint", "@lag/components/buttons.slint", "parts/clock.slint", "art/bg.png", "font.ttf"] {
            assert!(path_ok(&root, ok), "{ok}");
        }
        assert!(path_ok(&sub, "../theme/tokens.slint"), "up, but still inside the package");
        for bad in ["../x.slint", "C:/Windows/x.png", "/etc/x", "\\\\server\\x.png", "@lag/../../x", "parts/../../x", "@other/x"] {
            assert!(!path_ok(&root, bad), "{bad}");
        }
        assert!(!path_ok(&sub, "../../x.png"));
        let text = "// ported from \"Views/Old.axaml\"\nimport { MainVM } from \"@lag/api.slint\";\nimport \"fonts/a.ttf\";\nImage { source: @image-url(\"../secret.png\"); }";
        assert_eq!(referenced_paths(text), vec!["@lag/api.slint", "fonts/a.ttf", "../secret.png"], "comments are not imports");
    }

    /// docs/themes/API.md is the generated reference (LAG_UPDATE_API_DOC=1 rewrites it).
    #[test]
    fn the_api_reference_is_current() {
        let doc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/themes/API.md");
        if std::env::var_os("LAG_UPDATE_API_DOC").is_some() {
            std::fs::write(&doc, API_REFERENCE).unwrap();
        }
        let on_disk = std::fs::read_to_string(&doc).unwrap_or_default().replace("\r\n", "\n");
        assert!(on_disk == API_REFERENCE, "docs/themes/API.md is stale: run the tests with LAG_UPDATE_API_DOC=1");
        for must in ["### MainVM", "`save-replay`", "### SettingsRowData", "| `is-recording` | in property | `bool` |"] {
            assert!(API_REFERENCE.contains(must), "{must}");
        }
    }

    /// A new theme is only its manifest, yet it is the whole of Lag's interface: it compiles, draws every window
    /// except the tray menu, and its windows share Lag's globals (not copies of them).
    #[test]
    fn a_new_theme_from_lags_ui_works() {
        // LAG_KEEP_NEW_THEME=<themes folder>: also leave one there (for the pixel comparison with Lag's own UI)
        if let Some(keep) = std::env::var_os("LAG_KEEP_NEW_THEME") {
            new_from_builtin(Path::new(&keep), "Copy").unwrap();
        }
        let root = temp("newtheme");
        let t = new_from_builtin(&root, "Моя тема").unwrap();
        assert_eq!((t.id.as_str(), t.name.as_str()), ("моя-тема", "Моя тема"));
        let own: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(own, ["моя-тема.lag"], "one file, nothing of Lag's copied");
        let text = std::fs::read_to_string(t.location()).unwrap();
        assert!(text.starts_with(lag_theme_file::MAGIC) && !text.lines().any(|l| l.starts_with("#@file ") || l.starts_with("#@data ")), "{text}");
        let second = new_from_builtin(&root, "Моя тема").unwrap();
        assert_eq!((second.id.as_str(), second.name.as_str()), ("моя-тема-2", "Моя тема 2"), "never over an existing one, nor its name");
        ui_twin::load_builtin().unwrap();
        apply_dir(t.location()).unwrap();
        let windows = rt::loaded();
        for w in ["AppWindow", "ToastWindow", "StatsHud", "TrayMenuWindow"] {
            assert!(windows.iter().any(|x| x == w), "{w}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// A theme that changes one file gets everything else from Lag, and the newest of it: a file Lag changes later
    /// shows up in the theme; a file the theme drops comes back from Lag; the API stays Lag's even if the theme
    /// ships its own.
    #[test]
    fn a_theme_inherits_what_it_does_not_change() {
        let dir = temp("inherit");
        std::fs::write(dir.join("theme.toml"), "name = \"One file\"\n").unwrap();
        std::fs::create_dir_all(dir.join("theme")).unwrap();
        std::fs::create_dir_all(dir.join("api")).unwrap();
        let tokens = std::fs::read_to_string(ui_twin::builtin_dir().join("theme").join("tokens.slint")).unwrap();
        let changed = tokens.replacen("#22D3EE", "#C0392B", 1);
        assert_ne!(changed, tokens);
        std::fs::write(dir.join("theme").join("tokens.slint"), &changed).unwrap();
        std::fs::write(dir.join("api").join("globals.slint"), "export global MainVM { }").unwrap();
        let theme = read(&dir).unwrap();
        let build = sync(&theme).unwrap();
        assert_eq!(std::fs::read_to_string(build.join("theme").join("tokens.slint")).unwrap(), changed, "the theme's own file");
        let lag = |rel: &str| std::fs::read(ui_twin::builtin_dir().join(rel)).unwrap();
        assert_eq!(std::fs::read(build.join("views").join("settings.slint")).unwrap(), lag("views/settings.slint"), "Lag's file");
        assert_eq!(std::fs::read(build.join("api").join("globals.slint")).unwrap(), lag("api/globals.slint"), "Lag's API");
        assert!(!build.join("theme.toml").exists());
        ui_twin::load_builtin().unwrap();
        let (_, windows) = compile_theme(&theme).unwrap();
        assert!(windows.iter().any(|w| w == "AppWindow"), "{windows:?}");
        // dropped from the theme: Lag's again
        std::fs::remove_file(dir.join("theme").join("tokens.slint")).unwrap();
        sync(&theme).unwrap();
        assert_eq!(std::fs::read(build.join("theme").join("tokens.slint")).unwrap(), lag("theme/tokens.slint"));
        // a compile error names the theme's own path, not the build folder
        std::fs::write(dir.join("app.slint"), "export component AppWindow inherits Window { oops }").unwrap();
        let Err(e) = compile_theme(&theme) else { panic!("a broken theme compiled") };
        assert!(e.contains("app.slint") && !e.contains(&build.display().to_string()), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A `.lag` theme end to end: changed in its working folder, written back into its one file, read again; a
    /// folder theme made before 2.2 becomes one file; export / import of one file; rename inside the file.
    #[test]
    fn one_file_themes_round_trip() {
        let root = temp("onefile");
        let t = new_from_builtin(&root, "Файл").unwrap();
        materialize(&t).unwrap();
        std::fs::create_dir_all(t.dir.join("theme")).unwrap();
        let tokens = std::fs::read_to_string(ui_twin::builtin_dir().join("theme").join("tokens.slint")).unwrap();
        std::fs::write(t.dir.join("theme").join("tokens.slint"), tokens.replacen("#22D3EE", "#123456", 1)).unwrap();
        std::fs::create_dir_all(t.dir.join("art")).unwrap();
        std::fs::write(t.dir.join("art").join("bg.png"), [137u8, 80, 78, 71, 1, 2, 3]).unwrap();
        pack(&t).unwrap();
        let text = std::fs::read_to_string(t.location()).unwrap();
        assert!(text.contains("#@file theme/tokens.slint") && text.contains("#@data art/bg.png") && text.contains("#123456"));
        // fresh working folder from the file alone
        std::fs::remove_dir_all(&t.dir).unwrap();
        materialize(&t).unwrap();
        assert_eq!(std::fs::read(t.dir.join("art").join("bg.png")).unwrap(), [137u8, 80, 78, 71, 1, 2, 3]);
        ui_twin::load_builtin().unwrap();
        compile_theme(&t).unwrap();
        // rename: inside the file and its working folder
        let r = rename(&t, "Інша назва").unwrap();
        assert_eq!(read(t.location()).unwrap().name, "Інша назва");
        assert_eq!(read(&t.dir).unwrap().name, "Інша назва");
        assert_eq!(r.location(), t.location());
        // export: the same theme in one file elsewhere
        let out = std::env::temp_dir().join(format!("lag-export-{}.lag", std::process::id()));
        export(&r, &out).unwrap();
        assert_eq!(ThemeFile::parse(&std::fs::read_to_string(&out).unwrap()).unwrap().files.len(), 2);
        let _ = std::fs::remove_file(&out);
        // a folder theme from before 2.2
        let old = root.join("стара");
        std::fs::create_dir_all(old.join("views")).unwrap();
        std::fs::write(old.join("theme.toml"), "name = \"Стара\"
").unwrap();
        std::fs::write(old.join("views").join("x.slint"), "export component X {}
").unwrap();
        let tf = folder_to_file(&old).unwrap();
        assert_eq!(tf.manifest.name, "Стара");
        assert!(tf.files.contains_key("views/x.slint") && !tf.files.contains_key("theme.toml"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_theme_is_renamed_in_its_manifest() {
        let d = temp("rename");
        std::fs::write(d.join("theme.toml"), "name = \"Old\"\nauthor = \"me\"\napi = 1\n").unwrap();
        let theme = read(&d).unwrap();
        let t = rename(&theme, "  Нова назва ").unwrap();
        assert_eq!((t.name.as_str(), t.author.as_str()), ("Нова назва", "me"), "the other keys stay");
        assert!(rename(&theme, "   ").is_err());
        let _ = std::fs::remove_dir_all(&d);
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
        assert!(validate(&d).unwrap_err().contains("Desktop/a.png"), "names the path that leaves the theme");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_writes_one_file_and_zip_slip_is_refused() {
        use std::io::Write;
        let src = temp("src");
        std::fs::create_dir_all(src.join("art")).unwrap();
        std::fs::write(src.join("theme.toml"), "name = \"Rice\"
author = \"me\"
version = \"2\"
").unwrap();
        std::fs::write(src.join("app.slint"), "export component AppWindow inherits Window {}").unwrap();
        std::fs::write(src.join("art").join("bg.png"), [137u8, 80, 78, 71]).unwrap();
        let info = read(&src).unwrap();
        let file = std::env::temp_dir().join(format!("lag-rice-{}.lag", std::process::id()));
        export(&info, &file).unwrap();
        let tf = ThemeFile::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((tf.manifest.name.as_str(), tf.manifest.author.as_str(), tf.manifest.version.as_str()), ("Rice", "me", "2"));
        assert_eq!(tf.files["art/bg.png"], lag_theme_file::Content::Binary(vec![137u8, 80, 78, 71]));
        assert!(tf.files.contains_key("app.slint"));

        // a crafted .lagtheme (made before 2.2) that writes outside the folder
        let evil = std::env::temp_dir().join(format!("lag-evil-{}.lagtheme", std::process::id()));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&evil).unwrap());
        z.start_file("../../evil.slint", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
        let out2 = temp("out2");
        assert!(unpack(&evil, &out2).unwrap_err().contains("evil.slint"));
        for p in [src, out2] {
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
