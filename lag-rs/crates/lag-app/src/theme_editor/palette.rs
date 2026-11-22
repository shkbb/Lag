//! The theme's palette in the editor: every `color` / `brush` property of a global written in the theme's own files
//! (Lag's copy: `Theme` in `theme/tokens.slint`), grouped by the `// ── Section ──` comments above them. Lag's
//! colours read `is-neon ? A : is-dark ? B : C`: a change goes into the branch the chosen mode shows, so a theme
//! keeps its light and dark looks apart; "all modes" puts one colour in place of the whole expression.
use super::{parse_color, parse_text};
use i_slint_compiler::parser::SyntaxKind;
use slint::Color;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Token {
    pub path: PathBuf,
    /// start of the property's name: finds it again after an edit
    pub offset: u32,
    pub global: String,
    pub name: String,
    /// the section comment above it ("" before the first one)
    pub section: String,
    /// what the file says, without the `;`
    pub expr: String,
    /// where `expr` starts in the file
    pub expr_at: usize,
}

/// A section title from a comment line: `// ── Surfaces ──` -> "Surfaces"; a title under a `// ═══` rule too.
/// Notes in brackets and file names go: `Palette.axaml — Brand Accent` -> "Brand Accent",
/// `Theme mode ("dark" | ...) — matches App.SetTheme(..)` -> "Theme mode".
fn section_title(line: &str, after_rule: bool) -> Option<String> {
    let body = line.trim().strip_prefix("//")?;
    let raw = body.trim_matches(|c: char| c == '/' || c == '─' || c == '═' || c.is_whitespace());
    if raw.is_empty() || !(body.contains("──") || after_rule) {
        return None;
    }
    let mut plain = String::new();
    let mut depth = 0;
    for c in raw.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let parts: Vec<&str> = plain.split(" — ").map(str::trim).filter(|p| !p.is_empty()).collect();
    let title = parts.iter().find(|p| !p.contains('.')).or(parts.first()).copied().unwrap_or(raw);
    Some(title.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The palette written in one file.
pub(crate) fn tokens_in(text: &str, path: &Path) -> Vec<Token> {
    let root = parse_text(text, path);
    let mut out = Vec::new();
    for g in root.descendants().filter(|n| n.kind() == SyntaxKind::Component) {
        if !g.text().to_string().trim_start().starts_with("global") {
            continue;
        }
        let Some(global) = g.child_node(SyntaxKind::DeclaredIdentifier).map(|d| d.text().to_string().trim().to_owned()) else { continue };
        let Some(body) = g.child_node(SyntaxKind::Element) else { continue };
        // section titles in this global, by where they are
        let (start, end) = (usize::from(g.text_range().start()), usize::from(g.text_range().end()));
        let mut titles: Vec<(usize, String)> = Vec::new();
        let mut at = start;
        let mut after_rule = false;
        for line in text[start..end].split_inclusive('\n') {
            if let Some(t) = section_title(line, after_rule) {
                titles.push((at, t));
            }
            after_rule = line.contains("═══");
            at += line.len();
        }
        for d in body.children().filter(|n| n.kind() == SyntaxKind::PropertyDeclaration) {
            let ty = d.child_node(SyntaxKind::Type).map(|t| t.text().to_string().trim().to_owned()).unwrap_or_default();
            if ty != "color" && ty != "brush" {
                continue;
            }
            let (Some(id), Some(be)) = (d.child_node(SyntaxKind::DeclaredIdentifier), d.child_node(SyntaxKind::BindingExpression)) else {
                continue;
            };
            let raw = be.text().to_string();
            let lead = raw.len() - raw.trim_start().len();
            let expr = raw.trim().trim_end_matches(';').trim_end().to_owned();
            let offset = u32::from(id.text_range().start());
            let section = titles.iter().rev().find(|(p, _)| *p < offset as usize).map(|(_, t)| t.clone()).unwrap_or_default();
            out.push(Token {
                path: path.to_path_buf(),
                offset,
                global: global.clone(),
                name: id.text().to_string().trim().to_owned(),
                section,
                expr,
                expr_at: usize::from(be.text_range().start()) + lead,
            });
        }
    }
    out
}

/// The palette of every `.slint` file under `dir`, files in name order.
pub(crate) fn tokens(dir: &Path) -> Vec<Token> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "slint") {
                files.push(i_slint_compiler::pathutils::clean_path(&p));
            }
        }
    }
    files.sort();
    files.iter().flat_map(|p| std::fs::read_to_string(p).map(|t| tokens_in(&t, p)).unwrap_or_default()).collect()
}

/// The colour mode the palette edits: what `is-neon`, `is-dark`, `mode == "light"` are in it.
pub(crate) fn flag(mode: &str, name: &str) -> Option<bool> {
    match name {
        "is-neon" => Some(mode == "neon"),
        "is-dark" => Some(mode != "light"),
        "is-light" => Some(mode == "light"),
        _ => None,
    }
}

fn eval(cond: &str, mode: &str) -> Option<bool> {
    let c = cond.trim();
    if let Some(inner) = c.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        return eval(inner, mode);
    }
    if let Some(rest) = c.strip_prefix('!') {
        return eval(rest, mode).map(|v| !v);
    }
    if let Some((l, r)) = c.split_once("!=") {
        let (l, r) = (l.trim(), r.trim().trim_matches('"'));
        return (l == "mode").then(|| mode != r);
    }
    if let Some((l, r)) = c.split_once("==") {
        let (l, r) = (l.trim(), r.trim().trim_matches('"'));
        return (l == "mode").then(|| mode == r);
    }
    flag(mode, c)
}

/// The top-level `?` of `cond ? a : b` and the `:` that goes with it.
fn split_ternary(e: &str) -> Option<(usize, usize)> {
    let (mut depth, mut in_str, mut q, mut nest) = (0i32, false, None, 0);
    for (i, ch) in e.char_indices() {
        match ch {
            '"' => in_str = !in_str,
            _ if in_str => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '?' if depth == 0 => match q {
                None => q = Some(i),
                Some(_) => nest += 1,
            },
            ':' if depth == 0 && q.is_some() => {
                if nest == 0 {
                    return Some((q?, i));
                }
                nest -= 1;
            }
            _ => {}
        }
    }
    None
}

/// The byte range of `expr` that shows in `mode` (the whole of it when it is not a choice). None = it hangs on
/// something other than the mode (the editor cannot tell which part shows).
pub(crate) fn active_branch(expr: &str, mode: &str) -> Option<(usize, usize)> {
    let lead = expr.len() - expr.trim_start().len();
    let t = expr.trim();
    match split_ternary(t) {
        None => Some((lead, lead + t.len())),
        Some((q, c)) => {
            let (part, at) = if eval(&t[..q], mode)? { (&t[q + 1..c], q + 1) } else { (&t[c + 1..], c + 1) };
            active_branch(part, mode).map(|(s, e)| (lead + at + s, lead + at + e))
        }
    }
}

/// The colour a token shows in `mode`, following references to other colours of its global; a gradient shows
/// its first colour.
pub(crate) fn colour_of(t: &Token, all: &[Token], mode: &str) -> Option<Color> {
    fn go(t: &Token, all: &[Token], mode: &str, depth: u8) -> Option<Color> {
        let (s, e) = active_branch(&t.expr, mode)?;
        let v = t.expr[s..e].trim();
        if let Some(c) = parse_color(v) {
            return Some(c);
        }
        if v.starts_with("@linear-gradient") || v.starts_with("@radial-gradient") {
            let hex = &v[v.find('#')?..];
            let end = hex[1..].find(|c: char| !c.is_ascii_hexdigit()).map_or(hex.len(), |i| i + 1);
            return parse_color(&hex[..end]);
        }
        match v {
            "transparent" => return Some(Color::from_argb_u8(0, 0, 0, 0)),
            "white" => return Some(Color::from_rgb_u8(255, 255, 255)),
            "black" => return Some(Color::from_rgb_u8(0, 0, 0)),
            _ => {}
        }
        let name = v.strip_prefix(&format!("{}.", t.global)).unwrap_or(v);
        let next = all.iter().find(|x| x.global == t.global && x.name == name)?;
        (depth < 8).then(|| go(next, all, mode, depth + 1)).flatten()
    }
    go(t, all, mode, 0)
}

/// `Theme.on-accent` (a palette colour named in an element) -> what it shows in `mode`.
pub(crate) fn colour_named(expr: &str, all: &[Token], mode: &str) -> Option<Color> {
    let (global, name) = expr.trim().split_once('.')?;
    let t = all.iter().find(|t| t.global == global && t.name == name)?;
    colour_of(t, all, mode)
}

/// `FF4FA3` -> `#FF4FA3` (six or eight hex digits typed without the `#`); anything else as typed.
pub(crate) fn colour_input(input: &str) -> String {
    let t = input.trim();
    if (t.len() == 6 || t.len() == 8) && t.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("#{t}")
    } else {
        t.to_owned()
    }
}

/// Puts `value` into the token at `offset`: in the branch `mode` shows, or in place of the whole expression.
pub(crate) fn set_token(text: &str, path: &Path, offset: u32, mode: &str, all_modes: bool, value: &str) -> Result<String, String> {
    let t = tokens_in(text, path).into_iter().find(|t| t.offset == offset).ok_or("the colour moved in its file")?;
    let (s, e) = if all_modes {
        (0, t.expr.len())
    } else {
        active_branch(&t.expr, mode).ok_or_else(|| lag_core::i18n::tr("ThemeEditor_PaletteUnknown").to_owned())?
    };
    let mut out = text.to_owned();
    out.replace_range(t.expr_at + s..t.expr_at + e, value);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "export global Theme {\n    in-out property <string> mode: \"dark\";\n    out property <bool> is-dark: mode != \"light\";\n    out property <bool> is-neon: mode == \"neon\";\n    // ═══\n    // Palette.axaml — Brand Accent\n    // ═══\n    out property <color> accent: is-neon ? #FF4FA3 : is-dark ? #22D3EE : #A9722E;\n    // ── Surfaces ──\n    out property <color> card: is-dark ? #14161C : #F8F4EA;\n    out property <brush> cta: is-neon ? @linear-gradient(90deg, #FF4FA3 0%, #FF8A4C 100%) : accent;\n    out property <length> gap: 4px;\n}\nexport component W inherits Window { property <color> local: red; }\n";

    #[test]
    fn the_palette_is_read_with_its_sections() {
        let t = tokens_in(TEXT, Path::new("t.slint"));
        let names: Vec<_> = t.iter().map(|x| (x.name.as_str(), x.section.as_str())).collect();
        assert_eq!(names, [("accent", "Brand Accent"), ("card", "Surfaces"), ("cta", "Surfaces")]);
        assert_eq!(section_title("    // ── Theme mode (\"dark\" | \"light\") — matches App.SetTheme(string? mode) ──", false).as_deref(), Some("Theme mode"));
        assert_eq!(section_title("// ── Parallel Brush Set (DarkTheme.axaml + Views) ──", false).as_deref(), Some("Parallel Brush Set"));
        assert_eq!(section_title("// just a note", false), None);
        assert_eq!(t[1].expr, "is-dark ? #14161C : #F8F4EA");
        assert_eq!(&TEXT[t[1].expr_at..t[1].expr_at + t[1].expr.len()], t[1].expr);
        assert!(t.iter().all(|x| x.global == "Theme"));
    }

    #[test]
    fn the_branch_of_the_mode_is_found() {
        let e = "is-neon ? #FF4FA3 : is-dark ? #22D3EE : #A9722E";
        let part = |m| active_branch(e, m).map(|(s, x)| &e[s..x]);
        assert_eq!(part("neon"), Some("#FF4FA3"));
        assert_eq!(part("dark"), Some("#22D3EE"));
        assert_eq!(part("light"), Some("#A9722E"));
        assert_eq!(active_branch("#123456", "dark"), Some((0, 7)));
        assert_eq!(active_branch("hovered ? red : blue", "dark"), None);
        let g = "is-neon ? @linear-gradient(90deg, #FF4FA3 0%, #FF8A4C 100%) : accent";
        assert_eq!(active_branch(g, "neon").map(|(s, x)| &g[s..x]), Some("@linear-gradient(90deg, #FF4FA3 0%, #FF8A4C 100%)"));
        assert_eq!(active_branch("mode == \"light\" ? #FFF : #000", "light"), Some((18, 22)));
    }

    #[test]
    fn a_colour_changes_only_in_its_mode() {
        let p = Path::new("t.slint");
        let t = tokens_in(TEXT, p);
        let accent = t[0].offset;
        let light = set_token(TEXT, p, accent, "light", false, "#112233").unwrap();
        assert!(light.contains("accent: is-neon ? #FF4FA3 : is-dark ? #22D3EE : #112233;"), "{light}");
        let neon = set_token(&light, p, accent, "neon", false, "#445566").unwrap();
        assert!(neon.contains("accent: is-neon ? #445566 : is-dark ? #22D3EE : #112233;"), "{neon}");
        let all = set_token(&neon, p, accent, "dark", true, "#778899").unwrap();
        assert!(all.contains("property <color> accent: #778899;"), "{all}");
        // what each shows, through references and gradients
        let t = tokens_in(TEXT, p);
        assert_eq!(colour_of(&t[2], &t, "dark"), parse_color("#22D3EE"));
        assert_eq!(colour_of(&t[2], &t, "neon"), parse_color("#FF4FA3"));
        assert_eq!(colour_named("Theme.cta", &t, "dark"), parse_color("#22D3EE"));
        assert_eq!(colour_named("Other.cta", &t, "dark"), None);
        assert_eq!(colour_input("ff4fa3"), "#ff4fa3");
        assert_eq!(colour_input("accent"), "accent");
    }

    /// A copy of Lag's own interface: every colour of its palette is found in every mode, and a change in one
    /// mode still compiles.
    #[test]
    fn lags_own_palette_is_editable() {
        let root = std::env::temp_dir().join(format!("lag-palette-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let theme = crate::themes::new_from_builtin(&root, "Palette").unwrap();
        // the new theme holds nothing: its palette is Lag's, through the built theme
        let build = crate::themes::sync(&theme).unwrap();
        let all = tokens(&build);
        assert!(all.len() > 100, "{}", all.len());
        assert!(all.iter().any(|t| t.global == "Theme" && t.name == "accent" && t.section.contains("Brand Accent")));
        for mode in ["dark", "light", "neon"] {
            let unknown: Vec<_> = all.iter().filter(|t| active_branch(&t.expr, mode).is_none()).map(|t| &t.name).collect();
            assert!(unknown.is_empty(), "{mode}: {unknown:?}");
        }
        let accent = all.iter().find(|t| t.global == "Theme" && t.name == "accent").unwrap();
        assert_eq!(colour_of(accent, &all, "light"), parse_color("#A9722E"));
        // a change lands in the theme's own copy of the file
        let text = std::fs::read_to_string(&accent.path).unwrap();
        let after = set_token(&text, &accent.path, accent.offset, "light", false, "#123456").unwrap();
        let own = theme.dir.join(accent.path.strip_prefix(&build).unwrap());
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, after).unwrap();
        crate::themes::sync(&theme).unwrap();
        let again = tokens(&build);
        let accent2 = again.iter().find(|t| t.global == "Theme" && t.name == "accent").unwrap();
        assert_eq!(colour_of(accent2, &again, "light"), parse_color("#123456"));
        assert_eq!(colour_of(accent2, &again, "dark"), parse_color("#22D3EE"));
        crate::ui_twin::load_builtin().unwrap();
        crate::themes::compile_theme(&crate::themes::read(&theme.dir).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
