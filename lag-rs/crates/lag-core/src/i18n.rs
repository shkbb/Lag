//! Mirrors Core/Localizer.cs, Core/LanguageToFlagConverter.cs, App.axaml.cs and
//! ViewModels/SettingsViewModel.cs language options. Dictionaries mirror Assets/Langs/*.axaml.

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        LazyLock,
    },
};

/// Exact 30×20 flag drawing data from App.axaml, including the white/red/white Belarus flag.
#[derive(Debug, Deserialize)]
pub struct FlagLayer {
    pub fill: Option<String>,
    pub geometry: Option<String>,
    pub ellipse: Option<FlagEllipse>,
    pub stroke: Option<FlagStroke>,
}
#[derive(Debug, Deserialize)]
pub struct FlagEllipse {
    pub center: String,
    pub radius_x: f64,
    pub radius_y: f64,
}
#[derive(Debug, Deserialize)]
pub struct FlagStroke {
    pub color: String,
    pub thickness: f64,
}
static FLAGS: LazyLock<BTreeMap<String, Vec<FlagLayer>>> =
    LazyLock::new(|| serde_json::from_str(include_str!("flags.json")).expect("validated flag data"));
pub const FLAG_SIZE: (u32, u32) = (30, 20);
pub fn flag(code: &str) -> Option<&'static [FlagLayer]> {
    FLAGS.get(code).map(Vec::as_slice)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    pub code: &'static str,
    pub display: &'static str,
    /// Original DrawingImage resource identifier; no dependency on a UI framework.
    pub flag_resource: &'static str,
    pub flag_country: &'static str,
}

macro_rules! languages {
    ($(($code:literal, $display:literal, $country:literal)),+ $(,)?) => {
        pub const LANGUAGES: &[Language] = &[$(Language { code: $code, display: $display,
            flag_resource: concat!("Flag_", $code), flag_country: $country }),+];
        const JSON: &[&str] = &[$(include_str!(concat!("../langs/", $code, ".json"))),+];
    }
}
languages! {
    ("en", "English", "GB"), ("uk", "Українська", "UA"), ("de", "Deutsch", "DE"),
    ("fr", "Français", "FR"), ("be", "Беларуская", "BY"), ("lt", "Lietuvių", "LT"),
    ("et", "Eesti", "EE"), ("lv", "Latviešu", "LV"), ("fi", "Suomi", "FI"),
    ("sv", "Svenska", "SE"), ("no", "Norsk", "NO"), ("da", "Dansk", "DK"),
    ("nl", "Nederlands", "NL"), ("it", "Italiano", "IT"), ("es", "Español", "ES"),
    ("pt", "Português", "PT"), ("ja", "日本語", "JP"),
}
static TABLES: LazyLock<Vec<BTreeMap<String, String>>> = LazyLock::new(|| {
    JSON.iter().map(|s| serde_json::from_str(s).expect("validated embedded language dictionary")).collect()
});
static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn set_language(code: &str) {
    CURRENT.store(index(code), Ordering::Relaxed);
}
pub fn current_language() -> &'static Language {
    &LANGUAGES[CURRENT.load(Ordering::Relaxed)]
}
fn index(code: &str) -> usize {
    LANGUAGES.iter().position(|l| l.code == code).unwrap_or(0)
}

pub fn tr(key: &str) -> &str {
    lookup(CURRENT.load(Ordering::Relaxed), key)
}
pub fn tr_in<'a>(code: &str, key: &'a str) -> &'a str {
    lookup(index(code), key)
}
fn lookup(language: usize, key: &str) -> &str {
    TABLES[language].get(key).or_else(|| TABLES[0].get(key)).map_or(key, String::as_str)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatError;
impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid composite format or missing argument")
    }
}
impl std::error::Error for FormatError {}

/// C# Localizer.Format for string arguments (preformat numbers in the caller's culture).
/// Supports indexed placeholders, escaped braces and signed alignment; string arguments
/// ignore a format suffix, just as `string.Format` does. Covers every bundled template.
pub fn format(key: &str, args: &[&str]) -> Result<String, FormatError> {
    format_pattern(tr(key), args)
}
fn format_pattern(pattern: &str, args: &[&str]) -> Result<String, FormatError> {
    let mut output = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                output.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                output.push('}');
            }
            '{' => {
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some('{') | None => return Err(FormatError),
                        Some(c) => field.push(c),
                    }
                }
                let head = field.split_once(':').map_or(field.as_str(), |(h, _)| h);
                let (index, align) = head.split_once(',').map_or((head, None), |(i, a)| (i, Some(a)));
                let index = index.trim().parse::<usize>().map_err(|_| FormatError)?;
                let value = *args.get(index).ok_or(FormatError)?;
                let align = align.map_or(Ok(0), |a| a.trim().parse::<i32>().map_err(|_| FormatError))?;
                let padding = (align.unsigned_abs() as usize).saturating_sub(value.encode_utf16().count());
                if align > 0 {
                    output.extend(std::iter::repeat_n(' ', padding));
                }
                output.push_str(value);
                if align < 0 {
                    output.extend(std::iter::repeat_n(' ', padding));
                }
            }
            '}' => return Err(FormatError),
            c => output.push(c),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composite_formatting_matches_bundled_templates() {
        assert_eq!(format_pattern("{{{0}}} {1,4} {0,-3:any}", &["x", "😀"]).unwrap(), "{x}   😀 x  ");
        assert!(format_pattern("{2}", &["a"]).is_err());
        assert!(format_pattern("{0", &["a"]).is_err());
        assert!(format_pattern("}", &[]).is_err());
        for table in TABLES.iter() {
            for text in table.values() {
                assert!(format_pattern(text, &["first", "second"]).is_ok(), "{text}");
            }
        }
    }
    /// Since 2026-11-16 the wording is deliberately rewritten (owner-approved terminology, see
    /// lag-rs/docs/terminology), and Rust-only keys exist. What must still hold against the C# dictionaries:
    /// every C# key exists in every language, and each text keeps the C# placeholders ({0}, {1}, ...).
    #[test]
    fn dictionaries_cover_every_axaml_key_with_same_placeholders() {
        assert_eq!(LANGUAGES.len(), 17);
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../InstantReplay/Assets/Langs");
        for (i, language) in LANGUAGES.iter().enumerate() {
            assert!(!flag(language.code).unwrap().is_empty());
            for key in TABLES[0].keys() {
                assert!(TABLES[i].contains_key(key), "{} missing {key}", language.code);
            }
            let path = source.join(format!("{}.axaml", language.code));
            if !path.exists() {
                continue;
            }
            let xml = std::fs::read_to_string(path).unwrap();
            let doc = roxmltree::Document::parse(&xml).unwrap();
            let mut count = 0;
            for node in doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "String") {
                let key = node.attribute(("http://schemas.microsoft.com/winfx/2006/xaml", "Key")).unwrap();
                let value: String = node.children().filter_map(|n| n.text().filter(|_| n.is_text())).collect();
                let ours = TABLES[i].get(key).unwrap_or_else(|| panic!("{} missing C# key {key}", language.code));
                let placeholders = |s: &str| {
                    let mut v: Vec<String> =
                        (0..10).map(|n| format!("{{{n}}}")).filter(|p| s.contains(p.as_str())).collect();
                    v.sort();
                    v
                };
                assert_eq!(placeholders(ours), placeholders(&value), "{} {key}: placeholders changed", language.code);
                count += 1;
            }
            assert!(TABLES[i].len() >= count, "{} lost keys", language.code);
        }
    }
    #[test]
    fn exact_flags_match_source() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../InstantReplay/App.axaml");
        if !path.exists() {
            return;
        }
        let xml = std::fs::read_to_string(path).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        for language in LANGUAGES {
            let drawing = doc
                .descendants()
                .find(|n| {
                    n.is_element()
                        && n.tag_name().name() == "DrawingImage"
                        && n.attribute(("http://schemas.microsoft.com/winfx/2006/xaml", "Key"))
                            == Some(language.flag_resource)
                })
                .unwrap();
            let nodes: Vec<_> =
                drawing.descendants().filter(|n| n.is_element() && n.tag_name().name() == "GeometryDrawing").collect();
            let layers = flag(language.code).unwrap();
            assert_eq!(nodes.len(), layers.len());
            for (node, layer) in nodes.into_iter().zip(layers) {
                assert_eq!(node.attribute("Brush"), layer.fill.as_deref());
                assert_eq!(node.attribute("Geometry"), layer.geometry.as_deref());
                let ellipse = node.descendants().find(|n| n.is_element() && n.tag_name().name() == "EllipseGeometry");
                assert_eq!(ellipse.is_some(), layer.ellipse.is_some());
                if let (Some(e), Some(data)) = (ellipse, &layer.ellipse) {
                    assert_eq!(e.attribute("Center"), Some(data.center.as_str()));
                    assert_eq!(e.attribute("RadiusX").unwrap().parse::<f64>().unwrap(), data.radius_x);
                    assert_eq!(e.attribute("RadiusY").unwrap().parse::<f64>().unwrap(), data.radius_y);
                }
                let pen = node.descendants().find(|n| n.is_element() && n.tag_name().name() == "Pen");
                assert_eq!(pen.is_some(), layer.stroke.is_some());
                if let (Some(p), Some(data)) = (pen, &layer.stroke) {
                    assert_eq!(p.attribute("Brush"), Some(data.color.as_str()));
                    assert_eq!(p.attribute("Thickness").unwrap().parse::<f64>().unwrap(), data.thickness);
                }
            }
        }
    }
    #[test]
    fn fallback_and_switch() {
        set_language("uk");
        assert_eq!(current_language().code, "uk");
        assert_eq!(tr("Nav_Settings"), tr_in("uk", "Nav_Settings"));
        let missing = String::from("unknown_key");
        assert_eq!(tr(&missing), missing);
        set_language("UK");
        assert_eq!(current_language().code, "en");
        assert_eq!(tr("Nav_Settings"), "Settings");
    }
}
