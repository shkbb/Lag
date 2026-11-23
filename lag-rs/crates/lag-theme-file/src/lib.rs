//! The `.lag` theme file: a whole Lag theme in one UTF-8 text file, the same on Windows and Linux.
//!
//! ```text
//! #@lag-theme 1
//! # A Lag theme in one file. ...
//! name = "My theme"
//! author = "me"
//! version = "1.0"
//! api = 1
//!
//! #@file theme/tokens.slint
//! export global Theme { ... }
//! #@data art/bg.png
//! iVBORw0KGgoAAAANSUhEUgAA...
//! ```
//!
//! - The header (up to the first section) is TOML: `name` (required), `author`, `version`, `api`, `entry`.
//! - `#@file <path>`: a text file of the theme (`.slint`, `.svg`, `.md`, `.txt`), as it is, up to the next section.
//!   It replaces Lag's own file at that path (Lag's `ui` folder); every file not in the theme is Lag's.
//! - `#@data <path>`: a picture, a font or a video, as base64.
//! - Paths use `/`, are relative, never leave the theme (`..`, drives, roots) and keep their exact case.
//! - Line ends: LF or CRLF are read alike; written as LF. A leading BOM is ignored.
//! - A line of a text file that starts with `#@` is written with one more `#@` in front (`#@#@...`), so no file
//!   can contain a section marker; reading takes it off again. (A `.slint` line never starts with `#`.)
//!
//! No code runs from a theme: Lag compiles its `.slint` (a declarative UI language) and loads its pictures/fonts.
use base64::Engine;
use std::collections::BTreeMap;
use std::fmt;

/// The first line of every `.lag` file.
pub const MAGIC: &str = "#@lag-theme 1";
/// The file name extension (without the dot).
pub const EXTENSION: &str = "lag";

const FILE: &str = "#@file ";
const DATA: &str = "#@data ";
/// Text files (stored as they are) and binary files (base64) a theme may hold.
pub const TEXT_KINDS: &[&str] = &["slint", "svg", "md", "txt"];
pub const BINARY_KINDS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "apng", "ttf", "otf", "mp4", "webm"];
pub const MAX_FILES: usize = 2000;
pub const MAX_FILE: usize = 64 << 20;
pub const MAX_TOTAL: usize = 192 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub author: String,
    pub version: String,
    /// the theme API the theme was written for
    pub api: u32,
    /// the theme's main file (`app.slint`: Lag's own, unless the theme replaces it)
    pub entry: String,
}

impl Manifest {
    pub fn new(name: &str) -> Self {
        Manifest { name: name.to_owned(), author: String::new(), version: "1.0".into(), api: 1, entry: "app.slint".into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Binary(Vec<u8>),
}

impl Content {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Content::Text(t) => t.as_bytes(),
            Content::Binary(b) => b,
        }
    }
}

/// What can be wrong with a `.lag` file (Lag shows these in the user's language).
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// not a `.lag` theme (no `#@lag-theme 1` first line), or a newer format
    NotATheme,
    /// the header is not valid TOML / has no name
    Header(String),
    /// a path that leaves the theme, is empty, uses `\`, ...
    BadPath(String),
    /// a kind of file a theme cannot hold
    Kind(String),
    /// the same path twice
    Duplicate(String),
    /// base64 that does not decode
    BadData(String),
    /// a file over MAX_FILE (its path), or the whole theme over MAX_TOTAL (None)
    TooBig(Option<String>),
    TooMany,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotATheme => write!(f, "not a Lag theme file"),
            Error::Header(e) => write!(f, "the theme's header: {e}"),
            Error::BadPath(p) => write!(f, "{p}: the path leaves the theme or is not valid"),
            Error::Kind(p) => write!(f, "{p}: this kind of file cannot be in a theme"),
            Error::Duplicate(p) => write!(f, "{p}: twice in the theme"),
            Error::BadData(p) => write!(f, "{p}: the data is damaged"),
            Error::TooBig(Some(p)) => write!(f, "{p}: too big"),
            Error::TooBig(None) => write!(f, "the theme is too big"),
            Error::TooMany => write!(f, "the theme has too many files"),
        }
    }
}

impl std::error::Error for Error {}

fn kind_of(path: &str) -> Option<bool> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    if TEXT_KINDS.contains(&ext.as_str()) {
        Some(true)
    } else if BINARY_KINDS.contains(&ext.as_str()) {
        Some(false)
    } else {
        None
    }
}

/// A path a theme may hold: relative, `/`-separated, no `.`/`..`/empty parts, no drive, root, `\` or control
/// characters, a known kind of file.
pub fn check_path(path: &str) -> Result<(), Error> {
    let bad = || Error::BadPath(path.to_owned());
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err(bad());
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(bad());
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.trim() != part {
            return Err(bad());
        }
    }
    kind_of(path).map(|_| ()).ok_or_else(|| Error::Kind(path.to_owned()))
}

#[derive(serde::Deserialize)]
struct Header {
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

/// A whole theme: its manifest and its files by path.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeFile {
    pub manifest: Manifest,
    pub files: BTreeMap<String, Content>,
}

impl ThemeFile {
    /// A theme with no files: Lag's own interface under a name of its own.
    pub fn new(name: &str) -> Self {
        ThemeFile { manifest: Manifest::new(name), files: BTreeMap::new() }
    }

    /// Reads only the header (the theme list does not need the files).
    pub fn read_manifest(text: &str) -> Result<Manifest, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        if lines.next().map(str::trim_end) != Some(MAGIC) {
            return Err(Error::NotATheme);
        }
        let header: String = lines.take_while(|l| !is_marker(l)).map(|l| format!("{l}\n")).collect();
        parse_header(&header)
    }

    pub fn parse(text: &str) -> Result<Self, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let text = text.replace("\r\n", "\n");
        let mut lines = text.split('\n').peekable();
        if lines.next().map(str::trim_end) != Some(MAGIC) {
            return Err(Error::NotATheme);
        }
        let mut header = String::new();
        while let Some(l) = lines.peek() {
            if is_marker(l) {
                break;
            }
            header.push_str(l);
            header.push('\n');
            lines.next();
        }
        let manifest = parse_header(&header)?;
        let mut files = BTreeMap::new();
        let mut total = 0usize;
        while let Some(marker) = lines.next() {
            let (binary, path) = match (marker.strip_prefix(FILE), marker.strip_prefix(DATA)) {
                (Some(p), _) => (false, p.trim_end()),
                (_, Some(p)) => (true, p.trim_end()),
                _ => continue,
            };
            check_path(path)?;
            if files.contains_key(path) {
                return Err(Error::Duplicate(path.to_owned()));
            }
            let mut body: Vec<&str> = Vec::new();
            while let Some(l) = lines.peek() {
                if is_marker(l) {
                    break;
                }
                body.push(l);
                lines.next();
            }
            let content = if binary {
                let b64: String = body.concat().chars().filter(|c| !c.is_whitespace()).collect();
                let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|_| Error::BadData(path.to_owned()))?;
                Content::Binary(bytes)
            } else {
                // the text ends where the next section starts; the last line's break belongs to the file
                let mut t: String = body.iter().map(|l| unescape(l)).collect::<Vec<_>>().join("\n");
                if !t.is_empty() && body.last() == Some(&"") {
                    // `a\n` followed by a marker reads as ["a", ""]: the empty line is that break
                } else if !t.is_empty() {
                    t.push('\n');
                }
                Content::Text(t)
            };
            let size = content.bytes().len();
            if size > MAX_FILE {
                return Err(Error::TooBig(Some(path.to_owned())));
            }
            total += size;
            if total > MAX_TOTAL {
                return Err(Error::TooBig(None));
            }
            files.insert(path.to_owned(), content);
            if files.len() > MAX_FILES {
                return Err(Error::TooMany);
            }
        }
        Ok(ThemeFile { manifest, files })
    }

    /// The `.lag` text (LF line ends; the same bytes on every OS).
    pub fn to_text(&self) -> String {
        let m = &self.manifest;
        let q = |s: &str| toml::Value::String(s.to_owned()).to_string();
        let mut out = String::new();
        out.push_str(MAGIC);
        out.push('\n');
        out.push_str("# A Lag theme in one file. Each \"#@file <path>\" section replaces Lag's own file at that path\n");
        out.push_str("# (Lag's ui folder); \"#@data <path>\" holds a picture, font or video as base64. Everything that is\n");
        out.push_str("# not here comes from Lag itself, so Lag's updates reach it. Theme API: API.md in Lag's themes folder.\n");
        out.push_str(&format!("name = {}\nauthor = {}\nversion = {}\napi = {}\n", q(&m.name), q(&m.author), q(&m.version), m.api));
        if m.entry != "app.slint" {
            out.push_str(&format!("entry = {}\n", q(&m.entry)));
        }
        for (path, content) in &self.files {
            out.push('\n');
            match content {
                Content::Text(t) => {
                    out.push_str(FILE);
                    out.push_str(path);
                    out.push('\n');
                    for line in t.strip_suffix('\n').unwrap_or(t).split('\n') {
                        if line.starts_with("#@") {
                            out.push_str("#@");
                        }
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                Content::Binary(b) => {
                    out.push_str(DATA);
                    out.push_str(path);
                    out.push('\n');
                    let b64 = base64::engine::general_purpose::STANDARD.encode(b);
                    for chunk in b64.as_bytes().chunks(100) {
                        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
                        out.push('\n');
                    }
                }
            }
        }
        out
    }

    /// Puts a file in (a text kind as text, any other as binary). Err: a path the theme cannot hold.
    pub fn put(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        check_path(path)?;
        let content = match kind_of(path) {
            Some(true) => match String::from_utf8(bytes) {
                Ok(t) => Content::Text(t.replace("\r\n", "\n")),
                Err(e) => Content::Binary(e.into_bytes()),
            },
            _ => Content::Binary(bytes),
        };
        self.files.insert(path.to_owned(), content);
        Ok(())
    }

    pub fn remove(&mut self, path: &str) -> Option<Content> {
        self.files.remove(path)
    }
}

/// A section marker line. A blank line before a marker is the writer's spacing, not the previous file's.
fn is_marker(line: &str) -> bool {
    line.starts_with(FILE) || line.starts_with(DATA)
}

fn unescape(line: &str) -> &str {
    if line.starts_with("#@#@") {
        &line[2..]
    } else {
        line
    }
}

fn parse_header(header: &str) -> Result<Manifest, Error> {
    let h: Header = toml::from_str(header).map_err(|e| Error::Header(e.message().to_owned()))?;
    if h.name.trim().is_empty() {
        return Err(Error::Header("name is empty".into()));
    }
    let version = if h.version.is_empty() { "1.0".to_owned() } else { h.version };
    Ok(Manifest { name: h.name, author: h.author, version, api: h.api, entry: h.entry })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ThemeFile {
        let mut t = ThemeFile::new("Моя тема \"нова\"");
        t.manifest.author = "я".into();
        t.put("theme/tokens.slint", b"export global Theme {\n    out property <color> accent: #FF4FA3;\n}\n".to_vec()).unwrap();
        t.put("views/note.md", b"#@file evil.slint\nnot a marker\n\n".to_vec()).unwrap();
        t.put("art/bg.png", vec![137, 80, 78, 71, 0, 255, 1, 2]).unwrap();
        t.put("README.txt", b"no newline at the end".to_vec()).unwrap();
        t
    }

    #[test]
    fn a_theme_survives_writing_and_reading() {
        let t = sample();
        let text = t.to_text();
        assert!(text.starts_with(MAGIC));
        assert!(text.contains("#@file theme/tokens.slint\n") && text.contains("#@data art/bg.png\n"));
        assert!(text.contains("#@#@file evil.slint"), "a marker-like line inside a file is escaped");
        let back = ThemeFile::parse(&text).unwrap();
        assert_eq!(back.manifest, t.manifest);
        assert_eq!(back.files["theme/tokens.slint"], t.files["theme/tokens.slint"]);
        assert_eq!(back.files["art/bg.png"], t.files["art/bg.png"]);
        assert_eq!(back.files["views/note.md"], Content::Text("#@file evil.slint\nnot a marker\n\n".into()));
        assert_eq!(back.files["README.txt"], Content::Text("no newline at the end\n".into()), "LF added at the end");
        assert_eq!(back.files.len(), 4);
        // the same text again: stable
        assert_eq!(back.to_text(), text);
    }

    #[test]
    fn windows_line_ends_and_a_bom_read_alike() {
        let text = sample().to_text();
        let crlf = format!("\u{feff}{}", text.replace('\n', "\r\n"));
        assert_eq!(ThemeFile::parse(&crlf).unwrap(), ThemeFile::parse(&text).unwrap());
        assert_eq!(ThemeFile::read_manifest(&crlf).unwrap().name, "Моя тема \"нова\"");
    }

    #[test]
    fn an_empty_theme_is_just_its_header() {
        let t = ThemeFile::new("Порожня");
        let text = t.to_text();
        let back = ThemeFile::parse(&text).unwrap();
        assert!(back.files.is_empty());
        assert_eq!(back.manifest.name, "Порожня");
    }

    #[test]
    fn bad_files_are_refused() {
        assert_eq!(ThemeFile::parse("hello"), Err(Error::NotATheme));
        assert_eq!(ThemeFile::parse("#@lag-theme 2\nname = \"x\"\n"), Err(Error::NotATheme), "a newer format");
        assert!(matches!(ThemeFile::parse("#@lag-theme 1\nauthor = \"x\"\n"), Err(Error::Header(_))));
        for bad in ["../x.slint", "/etc/x.slint", "C:/x.slint", "a\\b.slint", "a//b.slint", "./a.slint", "a/../b.slint", ""] {
            assert!(matches!(check_path(bad), Err(Error::BadPath(_))), "{bad}");
        }
        assert!(matches!(check_path("run.exe"), Err(Error::Kind(_))));
        assert!(matches!(check_path("x.dll"), Err(Error::Kind(_))));
        let twice = format!("{MAGIC}\nname = \"x\"\n#@file a.slint\nA\n#@file a.slint\nB\n");
        assert_eq!(ThemeFile::parse(&twice), Err(Error::Duplicate("a.slint".into())));
        let broken = format!("{MAGIC}\nname = \"x\"\n#@data a.png\n!!!not base64\n");
        assert_eq!(ThemeFile::parse(&broken), Err(Error::BadData("a.png".into())));
        let outside = format!("{MAGIC}\nname = \"x\"\n#@file ../../evil.slint\nx\n");
        assert!(matches!(ThemeFile::parse(&outside), Err(Error::BadPath(_))));
    }

    #[test]
    fn case_is_kept() {
        let mut t = ThemeFile::new("x");
        t.put("Art/BG.png", vec![1]).unwrap();
        t.put("art/bg.png", vec![2]).unwrap();
        let back = ThemeFile::parse(&t.to_text()).unwrap();
        assert_eq!(back.files.len(), 2, "Linux keeps both");
    }
}
