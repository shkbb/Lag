//! Theme editor (docs/spec/themes.md, phase 4): `Lag.exe --theme-editor <theme folder>`. The theme runs live inside
//! the editor window (a component factory); a click picks the element under the pointer (Slint's own source-to-item
//! mapping: every element of the theme's files is located on screen); the panel lists every property the element
//! has (from the compiler's type of it) with what the file says now; a change is written into the theme's `.slint`
//! file as a text edit, compiled, and swapped in. An edit that does not compile is undone in the file at once and
//! the compiler's message is shown. Undo / redo keep whole-file texts.
use crate::{startup, themes, ui_twin, ComponentHandle, EditorCrumb, EditorProp, EditorRect, ThemeEditorWindow, Tr};
use i_slint_compiler::langtype::Type;
use i_slint_compiler::object_tree::ElementRc;
use i_slint_compiler::parser::{SyntaxKind, SyntaxNode};
use slint::{Color, ComponentFactory, ModelRc, SharedString, VecModel};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

pub fn args() -> Option<PathBuf> {
    let a: Vec<String> = std::env::args().collect();
    let i = a.iter().position(|x| x == "--theme-editor")?;
    a.get(i + 1).map(PathBuf::from)
}

/// One element written in the theme's files.
#[derive(Clone, Debug, PartialEq)]
struct Source {
    path: PathBuf,
    /// start of the element's type name (`Rectangle` in `Rectangle {`): how Slint finds its items
    offset: u32,
    /// `Rectangle`, `save := Rectangle`
    label: String,
    line: usize,
}

fn parse_text(text: &str, path: &Path) -> SyntaxNode {
    let mut diag = i_slint_compiler::diagnostics::BuildDiagnostics::default();
    i_slint_compiler::parser::parse(text.to_owned(), Some(path), &mut diag)
}

fn name_offset(el: &SyntaxNode) -> Option<u32> {
    el.child_node(SyntaxKind::QualifiedName).map(|q| q.text_range().start().into())
}

fn label_of(el: &SyntaxNode) -> String {
    // the root of `component AppWindow inherits Window`: its name, not the base type's
    if let Some(c) = el.parent().filter(|p| p.kind() == SyntaxKind::Component) {
        if let Some(id) = c.child_node(SyntaxKind::DeclaredIdentifier) {
            return id.text().to_string().trim().to_owned();
        }
    }
    let name = el.child_node(SyntaxKind::QualifiedName).map(|q| q.text().to_string()).unwrap_or_default();
    let id = el
        .parent()
        .filter(|p| p.kind() == SyntaxKind::SubElement)
        .and_then(|p| p.child_token(SyntaxKind::Identifier))
        .map(|t| format!("{} := ", t.text()))
        .unwrap_or_default();
    format!("{id}{}", name.trim())
}

fn source_of(el: &SyntaxNode, path: &Path, text: &str) -> Option<Source> {
    let offset = name_offset(el)?;
    let line = text[..offset as usize].matches('\n').count() + 1;
    Some(Source { path: path.to_path_buf(), offset, label: label_of(el), line })
}

fn find_element(root: &SyntaxNode, offset: u32) -> Option<SyntaxNode> {
    root.descendants().find(|n| n.kind() == SyntaxKind::Element && name_offset(n) == Some(offset))
}

/// Every element in every `.slint` file of the theme.
fn sources(dir: &Path) -> Vec<Source> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|x| x != "slint") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let path = i_slint_compiler::pathutils::clean_path(&path);
            let root = parse_text(&text, &path);
            out.extend(root.descendants().filter(|n| n.kind() == SyntaxKind::Element).filter_map(|el| source_of(&el, &path, &text)));
        }
    }
    out
}

/// Sets `name: value;` on the element at `offset` (replaces the binding or adds it).
pub(crate) fn set_binding(text: &str, path: &Path, offset: u32, name: &str, value: &str) -> Result<String, String> {
    let root = parse_text(text, path);
    let el = find_element(&root, offset).ok_or("the element moved in its file; pick it again")?;
    let existing = binding_node(&el, name);
    let mut out = text.to_owned();
    match existing {
        Some(b) => {
            let r = b.text_range();
            out.replace_range(usize::from(r.start())..usize::from(r.end()), &format!("{name}: {value};"));
        }
        None => {
            // right after the opening brace, on a line of its own, indented one step deeper than the element
            let brace = el.child_token(SyntaxKind::LBrace).ok_or("no { in the element")?;
            let at = usize::from(brace.text_range().end());
            let line_start = text[..offset as usize].rfind('\n').map_or(0, |i| i + 1);
            let indent: String = text[line_start..].chars().take_while(|c| c.is_whitespace() && *c != '\n').collect();
            out.insert_str(at, &format!("\n{indent}    {name}: {value};"));
        }
    }
    Ok(out)
}

/// Removes `name: ...;` from the element at `offset` (back to the element's default).
pub(crate) fn remove_binding(text: &str, path: &Path, offset: u32, name: &str) -> Result<String, String> {
    let root = parse_text(text, path);
    let el = find_element(&root, offset).ok_or("the element moved in its file; pick it again")?;
    let b = binding_node(&el, name).ok_or("not set")?;
    let (mut s, mut e) = (usize::from(b.text_range().start()), usize::from(b.text_range().end()));
    // the whole line when the binding is alone on it
    let line_start = text[..s].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[e..].find('\n').map_or(text.len(), |i| e + i);
    if text[line_start..s].trim().is_empty() && text[e..line_end].trim().is_empty() {
        s = line_start.saturating_sub(1);
        e = line_end;
    }
    let mut out = text.to_owned();
    out.replace_range(s..e, "");
    Ok(out)
}

/// The text block an element occupies among its siblings: `id := X { }`, `for ..: X { }`, `if ..: X { }`, or the
/// element itself. None for a component's root (it cannot be moved or removed).
fn block_of(el: &SyntaxNode) -> Option<SyntaxNode> {
    let mut b = el.parent().filter(|p| p.kind() == SyntaxKind::SubElement)?;
    while let Some(p) = b.parent().filter(|p| matches!(p.kind(), SyntaxKind::RepeatedElement | SyntaxKind::ConditionalElement)) {
        b = p;
    }
    Some(b)
}

fn is_block(n: &SyntaxNode) -> bool {
    matches!(n.kind(), SyntaxKind::SubElement | SyntaxKind::RepeatedElement | SyntaxKind::ConditionalElement)
}

fn range(n: &SyntaxNode) -> (usize, usize) {
    (usize::from(n.text_range().start()), usize::from(n.text_range().end()))
}

fn indent_at(text: &str, at: usize) -> String {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..].chars().take_while(|c| c.is_whitespace() && *c != '\n').collect()
}

/// A structural change to the element at `offset`. Returns the new text and the offset of the element to select
/// afterwards (None = its parent).
pub(crate) fn restructure(text: &str, path: &Path, offset: u32, op: &str) -> Result<(String, Option<u32>), String> {
    let root = parse_text(text, path);
    let el = find_element(&root, offset).ok_or("the element moved in its file; pick it again")?;
    let mut out = text.to_owned();
    match op {
        "delete" => {
            let b = block_of(&el).ok_or("the root of the window cannot be removed")?;
            let (mut s, mut e) = range(&b);
            let line_start = text[..s].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[e..].find('\n').map_or(text.len(), |i| e + i);
            if text[line_start..s].trim().is_empty() && text[e..line_end].trim().is_empty() {
                s = line_start.saturating_sub(1);
                e = line_end;
            }
            out.replace_range(s..e, "");
            Ok((out, None))
        }
        "duplicate" => {
            let b = block_of(&el).ok_or("the root of the window cannot be copied")?;
            let (s, e) = range(&b);
            // the copy without the `id :=` (two elements cannot share an id)
            let (cs, ce) = if b.kind() == SyntaxKind::SubElement { range(&el) } else { (s, e) };
            let copy = rename_ids(&text[cs..ce], text);
            let indent = indent_at(text, s);
            out.insert_str(e, &format!("\n{indent}{copy}"));
            let new_at = e + 1 + indent.len() + (offset as usize - cs);
            Ok((out, Some(new_at as u32)))
        }
        "up" | "down" => {
            let b = block_of(&el).ok_or("the root of the window has no neighbours")?;
            let parent = b.parent().ok_or("no parent")?;
            let siblings: Vec<SyntaxNode> = parent.children().filter(is_block).collect();
            let i = siblings.iter().position(|n| n.text_range() == b.text_range()).ok_or("not found")?;
            let j = if op == "up" { i.checked_sub(1) } else { (i + 1 < siblings.len()).then_some(i + 1) };
            let j = j.ok_or(if op == "up" { "already the first" } else { "already the last" })?;
            let (first, second) = if i < j { (&siblings[i], &siblings[j]) } else { (&siblings[j], &siblings[i]) };
            let ((a0, a1), (b0, b1)) = (range(first), range(second));
            let (ta, mid, tb) = (&text[a0..a1], &text[a1..b0], &text[b0..b1]);
            out = format!("{}{tb}{mid}{ta}{}", &text[..a0], &text[b1..]);
            // where the moved element's name sits now
            let (bs, _) = range(&b);
            let inside = offset as usize - bs;
            let new_block_start = if i < j { a0 + tb.len() + mid.len() } else { a0 };
            Ok((out, Some((new_block_start + inside) as u32)))
        }
        "add-rect" | "add-text" => {
            let close = el.child_token(SyntaxKind::RBrace).ok_or("no } in the element")?;
            let at = usize::from(close.text_range().start());
            let indent = indent_at(text, offset as usize);
            let snippet = if op == "add-rect" {
                "Rectangle { x: 24px; y: 24px; width: 180px; height: 90px; background: #FF4FA3; border-radius: 14px; }"
            } else {
                "Text { x: 24px; y: 24px; text: \"Text\"; color: #FFFFFF; font-size: 20px; font-weight: 700; }"
            };
            // keep `}` on its own line where it was
            let before_close = text[..at].trim_end_matches([' ', '\t']);
            let ins = format!("{indent}    {snippet}\n{indent}");
            let start = before_close.len();
            let lead = if before_close.ends_with('\n') { "" } else { "\n" };
            out.replace_range(start..at, &format!("{lead}{ins}"));
            let name_at = start + lead.len() + indent.len() + 4;
            Ok((out, Some(name_at as u32)))
        }
        _ => Err(format!("unknown change {op}")),
    }
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// A copy of an element: every `id :=` inside it gets a new id that the file does not use yet (`touch` ->
/// `touch-2`), and the copy's own references to those ids follow (`touch.pressed` -> `touch-2.pressed`).
fn rename_ids(copy: &str, whole: &str) -> String {
    let mut ids: Vec<String> = Vec::new();
    for (i, _) in copy.match_indices(":=") {
        let before = copy[..i].trim_end();
        let start = before.rfind(|c: char| !ident_char(c)).map_or(0, |j| j + before[j..].chars().next().map_or(1, char::len_utf8));
        let id = &before[start..];
        if !id.is_empty() && !ids.iter().any(|x| x == id) {
            ids.push(id.to_owned());
        }
    }
    let mut out = copy.to_owned();
    for id in ids {
        let used = |cand: &str| whole.contains(&format!("{cand} :=")) || whole.contains(&format!("{cand}:="));
        let new = (2..).map(|n| format!("{id}-{n}")).find(|c| !used(c)).unwrap_or_default();
        // whole-word replacement
        let mut res = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(&id) {
            let before_ok = rest[..i].chars().last().is_none_or(|c| !ident_char(c));
            let after_ok = rest[i + id.len()..].chars().next().is_none_or(|c| !ident_char(c));
            res.push_str(&rest[..i]);
            res.push_str(if before_ok && after_ok { &new } else { &id });
            rest = &rest[i + id.len()..];
        }
        res.push_str(rest);
        out = res;
    }
    out
}

/// Whether the element's place is decided by a layout (its parent is a `...Layout`): it moves by order there.
fn in_layout(el: &SyntaxNode) -> bool {
    let mut p = el.parent();
    while let Some(n) = p {
        if n.kind() == SyntaxKind::Element {
            return n.child_node(SyntaxKind::QualifiedName).is_some_and(|q| q.text().to_string().trim().ends_with("Layout"));
        }
        p = n.parent();
    }
    false
}

fn binding_node(el: &SyntaxNode, name: &str) -> Option<SyntaxNode> {
    el.children().find(|b| {
        matches!(b.kind(), SyntaxKind::Binding | SyntaxKind::TwoWayBinding)
            && b.child_token(SyntaxKind::Identifier).is_some_and(|t| t.text() == name)
    })
}

/// What the file says for `name` on this element (the expression, without `name:` and `;`).
fn binding_text(el: &SyntaxNode, name: &str) -> Option<String> {
    let b = binding_node(el, name)?;
    let t = b.text().to_string();
    let after = t.split_once(':').map(|(_, v)| v).or_else(|| t.split_once("<=>").map(|(_, v)| v))?;
    Some(after.trim().trim_end_matches(';').trim().to_owned())
}

struct Edit {
    path: PathBuf,
    before: String,
    after: String,
}

struct State {
    dir: PathBuf,
    theme: themes::ThemeInfo,
    /// the live theme inside the editor
    instance: Rc<RefCell<Option<ComponentInstance>>>,
    /// the theme's elements with their types (before the compiler's later passes)
    raw: Option<i_slint_compiler::typeloader::TypeLoader>,
    sources: Vec<Source>,
    selected: Option<Source>,
    page: Rc<std::cell::Cell<i32>>,
    filter: String,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

const PAGES: [&str; 4] = ["settings", "library", "player", "editor"];

/// Sample state for the theme in the editor: texts translated, a recording in progress, the chosen page.
fn fill(i: &ComponentInstance, page: i32) {
    let _ = i.set_global_callback("Tr", "t", |a| match a.first() {
        Some(Value::String(k)) => Value::String(lag_core::i18n::tr(k).into()),
        _ => Value::String(Default::default()),
    });
    let set = |n: &str, v: Value| {
        let _ = i.set_global_property("MainVM", n, v);
    };
    set("status-text", Value::String(lag_core::i18n::tr("Status_Ready").into()));
    set("buffer-status", Value::String("1:24".into()));
    set("buffer-fill-percent", Value::Number(70.0));
    set("current-page", Value::EnumerationValue("Page".into(), PAGES[page.clamp(0, 3) as usize].into()));
}

fn factory(def: ComponentDefinition, slot: Rc<RefCell<Option<ComponentInstance>>>, page: Rc<std::cell::Cell<i32>>) -> Value {
    Value::ComponentFactory(ComponentFactory::new(move |ctx| {
        let inst = def.create_embedded(ctx).ok()?;
        fill(&inst, page.get());
        *slot.borrow_mut() = Some(slint::ComponentHandle::clone_strong(&inst));
        Some(inst)
    }))
}

/// Compiles the theme and puts it on the canvas. Err = the compiler's text.
fn load(ui: &ThemeEditorWindow, st: &mut State) -> Result<(), String> {
    let (defs, _) = themes::compile_theme(&st.theme)?;
    let def = defs.into_iter().find(|d| d.name() == "AppWindow").ok_or("the theme has no AppWindow")?;
    st.raw = def.raw_type_loader();
    st.sources = sources(&st.dir);
    ui.instance().set_property("canvas", factory(def, st.instance.clone(), st.page.clone())).map_err(|e| format!("{e:?}"))?;
    Ok(())
}

/// The theme on the canvas now.
fn live(st: &State) -> Option<ComponentInstance> {
    st.instance.borrow().as_ref().map(slint::ComponentHandle::clone_strong)
}

fn rects(inst: &ComponentInstance, s: &Source) -> Vec<EditorRect> {
    inst.component_positions(&s.path, s.offset)
        .into_iter()
        .map(|h| EditorRect { x: h.rect.origin.x, y: h.rect.origin.y, width: h.rect.size.width, height: h.rect.size.height })
        .collect()
}

/// The innermost element under (x, y); a second click on the same spot picks the one around it.
fn pick(st: &State, x: f32, y: f32) -> Option<Source> {
    let inst = live(st)?;
    let mut hits: Vec<(f32, Source)> = st
        .sources
        .iter()
        .filter_map(|s| {
            let r = rects(&inst, s).into_iter().find(|r| x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height)?;
            Some((r.width * r.height, s.clone()))
        })
        .collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    let current = st.selected.as_ref().and_then(|c| hits.iter().position(|(_, s)| s == c));
    let i = current.map_or(0, |i| (i + 1) % hits.len().max(1));
    hits.into_iter().nth(i).map(|(_, s)| s)
}

/// The compiler's element for a source element (its type knows every property).
fn raw_element(st: &State, s: &Source) -> Option<ElementRc> {
    fn visit(e: &ElementRc, path: &Path, offset: u32) -> Option<ElementRc> {
        let found = e.borrow().debug.iter().any(|d| {
            d.node.source_file.path() == path
                && d.node.QualifiedName().is_some_and(|q| u32::from(q.text_range().start()) == offset)
        });
        if found {
            return Some(e.clone());
        }
        let children = e.borrow().children.clone();
        children.iter().find_map(|c| visit(c, path, offset))
    }
    let doc = st.raw.as_ref()?.get_document(&s.path)?;
    doc.inner_components.iter().find_map(|c| visit(&c.root_element, &s.path, s.offset))
}

/// Properties shown first, in this order (the rest follow: set ones first, then by name).
const COMMON: &[&str] = &[
    "x", "y", "width", "height", "background", "color", "text", "font-size", "font-weight", "font-family",
    "border-radius", "border-width", "border-color", "opacity", "visible", "source", "image-fit", "colorize",
    "horizontal-alignment", "vertical-alignment", "padding", "spacing", "alignment", "clip", "drop-shadow-blur",
    "drop-shadow-color", "drop-shadow-offset-x", "drop-shadow-offset-y", "rotation-angle", "z",
];

fn kind_of(t: &Type) -> Option<(i32, String, Vec<String>)> {
    Some(match t {
        Type::Color => (1, "color".into(), vec![]),
        Type::Brush => (1, "brush".into(), vec![]),
        Type::LogicalLength | Type::PhysicalLength | Type::Rem => (2, "length".into(), vec![]),
        Type::Float32 => (3, "float".into(), vec![]),
        Type::Int32 => (3, "int".into(), vec![]),
        Type::Percent => (3, "percent".into(), vec![]),
        Type::Angle => (3, "angle".into(), vec![]),
        Type::Duration => (3, "duration".into(), vec![]),
        Type::Bool => (4, "bool".into(), vec![]),
        Type::String => (5, "string".into(), vec![]),
        Type::Image => (7, "image".into(), vec![]),
        Type::Enumeration(e) => (6, e.name.to_string(), e.values.iter().map(|v| v.to_string()).collect()),
        Type::Callback(_) | Type::Function(_) | Type::InferredCallback | Type::InferredProperty | Type::Invalid | Type::Void => return None,
        Type::ElementReference | Type::LayoutCache | Type::ArrayOfU16 | Type::ComponentFactory | Type::DataTransfer => return None,
        other => (0, other.to_string(), vec![]),
    })
}

/// `#3DDCFF` / `#3DDCFF80` / `#3DF` -> the colour, for the swatch.
fn parse_color(expr: &str) -> Option<Color> {
    let h = expr.trim().strip_prefix('#')?;
    let v = |s: &str| u8::from_str_radix(s, 16).ok();
    let full: String = if h.len() == 3 || h.len() == 4 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_owned() };
    match full.len() {
        6 => Some(Color::from_rgb_u8(v(&full[0..2])?, v(&full[2..4])?, v(&full[4..6])?)),
        8 => Some(Color::from_argb_u8(v(&full[6..8])?, v(&full[0..2])?, v(&full[2..4])?, v(&full[4..6])?)),
        _ => None,
    }
}

fn props_of(st: &State, s: &Source) -> Vec<EditorProp> {
    let Some(el) = raw_element(st, s) else { return Vec::new() };
    let text = std::fs::read_to_string(&s.path).unwrap_or_default();
    let root = parse_text(&text, &s.path);
    let node = find_element(&root, s.offset);
    let mut list = el.borrow().base_type.property_list();
    // properties the element declares itself (`in property <..> x`)
    list.extend(el.borrow().property_declarations.iter().map(|(k, d)| (k.clone(), d.property_type.clone())));
    let filter = st.filter.trim().to_lowercase();
    let mut props: Vec<EditorProp> = list
        .into_iter()
        .filter(|(n, _)| !n.starts_with("accessible-") && (filter.is_empty() || n.to_lowercase().contains(&filter)))
        .filter_map(|(name, t)| {
            let (kind, label, options) = kind_of(&t)?;
            let value = node.as_ref().and_then(|n| binding_text(n, &name)).unwrap_or_default();
            let swatch = parse_color(&value).unwrap_or(Color::from_argb_u8(0, 0, 0, 0));
            Some(EditorProp {
                name: name.to_string().into(),
                kind,
                type_label: label.into(),
                bound: !value.is_empty(),
                value: value.into(),
                options: ModelRc::new(VecModel::from(options.into_iter().map(SharedString::from).collect::<Vec<_>>())),
                swatch,
            })
        })
        .collect();
    props.sort_by_key(|p| {
        let common = COMMON.iter().position(|c| *c == p.name.as_str()).unwrap_or(usize::MAX);
        (common, !p.bound, p.name.to_string())
    });
    props.dedup_by(|a, b| a.name == b.name);
    props
}

/// What the user typed -> a Slint expression for this kind of property.
fn expression(kind: i32, input: &str) -> String {
    let t = input.trim();
    let plain_number = !t.is_empty() && t.parse::<f64>().is_ok();
    match kind {
        2 if plain_number => format!("{t}px"),
        5 if !(t.starts_with('"') || t.contains('(') || t.contains("Tr.") || t.contains('.') && !t.contains(' ')) => {
            format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
        }
        7 if !t.starts_with('@') && !t.is_empty() => format!("@image-url(\"{t}\")"),
        _ => t.to_owned(),
    }
}

/// The element and the ones around it in its file (outermost first).
fn crumbs(s: &Source) -> Vec<Source> {
    let Ok(text) = std::fs::read_to_string(&s.path) else { return Vec::new() };
    let root = parse_text(&text, &s.path);
    let Some(mut node) = find_element(&root, s.offset) else { return Vec::new() };
    let mut chain = vec![];
    if let Some(src) = source_of(&node, &s.path, &text) {
        chain.push(src);
    }
    while let Some(p) = node.parent() {
        if p.kind() == SyntaxKind::Element {
            if let Some(src) = source_of(&p, &s.path, &text) {
                chain.push(src);
            }
        }
        node = p;
    }
    chain.reverse();
    chain
}

/// Writes `after` into `path`, compiles; an error puts the file back and returns the compiler's message.
fn change(ui: &ThemeEditorWindow, st: &mut State, path: &Path, after: String, record: bool) -> Result<(), String> {
    let before = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if before == after {
        return Ok(());
    }
    std::fs::write(path, &after).map_err(|e| e.to_string())?;
    if let Err(e) = load(ui, st) {
        let _ = std::fs::write(path, &before);
        let _ = load(ui, st);
        let rel = format!("{}{}", st.dir.display(), std::path::MAIN_SEPARATOR);
        return Err(e.replace(&rel, ""));
    }
    if record {
        st.undo.push(Edit { path: path.to_path_buf(), before, after });
        st.redo.clear();
    }
    Ok(())
}

/// After a change the element may sit a line lower or higher in its file: find it again.
fn reselect(st: &mut State, old: &Source) {
    st.selected = st
        .sources
        .iter()
        .filter(|s| s.path == old.path && s.label == old.label)
        .min_by_key(|s| s.line.abs_diff(old.line))
        .cloned();
}

fn refresh(ui: &ThemeEditorWindow, st: &State) {
    let inst = live(st);
    match (&st.selected, inst) {
        (Some(s), Some(i)) => {
            let rel = s.path.strip_prefix(i_slint_compiler::pathutils::clean_path(&st.dir)).unwrap_or(&s.path).display().to_string();
            ui.set_selection(ModelRc::new(VecModel::from(rects(&i, s))));
            ui.set_selected_name(s.label.clone().into());
            ui.set_selected_where(format!("{rel}:{}", s.line).into());
            let chain = crumbs(s);
            let crumbs: Vec<EditorCrumb> = chain
                .iter()
                .enumerate()
                .map(|(i, c)| EditorCrumb { label: c.label.clone().into(), index: i as i32, current: c == s })
                .collect();
            ui.set_crumbs(ModelRc::new(VecModel::from(crumbs)));
            ui.set_props(ModelRc::new(VecModel::from(props_of(st, s))));
        }
        _ => {
            ui.set_selection(ModelRc::default());
            ui.set_selected_name("".into());
            ui.set_selected_where("".into());
            ui.set_crumbs(ModelRc::default());
            ui.set_props(ModelRc::default());
        }
    }
    ui.set_can_undo(!st.undo.is_empty());
    ui.set_can_redo(!st.redo.is_empty());
}

/// Refreshes once the swapped-in theme has drawn (its element rectangles are known only then).
fn refresh_soon(ui: &ThemeEditorWindow, st: &Rc<RefCell<State>>) {
    let (weak, st) = (ui.as_weak(), st.clone());
    slint::Timer::single_shot(Duration::from_millis(120), move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &st.borrow());
        }
    });
}

fn report(ui: &ThemeEditorWindow, result: Result<String, String>) {
    match result {
        Ok(m) => {
            ui.set_status(m.into());
            ui.set_status_error(false);
        }
        Err(e) => {
            ui.set_status(e.into());
            ui.set_status_error(true);
        }
    }
}

/// Applies a text change to the selected element's file (set or reset a property).
fn edit_selected(
    ui: &ThemeEditorWindow,
    s: &Rc<RefCell<State>>,
    what: impl FnOnce(&State, &Source, &str) -> Result<(String, String), String>,
) {
    let result = {
        let mut st = s.borrow_mut();
        let Some(sel) = st.selected.clone() else { return };
        let r = std::fs::read_to_string(&sel.path)
            .map_err(|e| e.to_string())
            .and_then(|t| what(&st, &sel, &t))
            .and_then(|(after, message)| change(ui, &mut st, &sel.path, after, true).map(|_| message));
        reselect(&mut st, &sel);
        r
    };
    report(ui, result);
    refresh_soon(ui, s);
}

pub fn run(dir: PathBuf) {
    if log::set_logger(&crate::STDERR_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let settings = startup::load_settings();
    lag_core::i18n::set_language(settings.language.as_deref().unwrap_or("en"));
    if let Err(e) = ui_twin::load_builtin() {
        startup::fatal_message(&format!("Lag could not load its interface:\n\n{e}"));
        return;
    }
    let theme = match themes::read(&dir) {
        Ok(t) => t,
        Err(e) => {
            startup::fatal_message(&format!("Not a Lag theme folder:\n\n{e}"));
            return;
        }
    };
    let Ok(ui) = ThemeEditorWindow::new() else { return };
    ui.global::<Tr>().on_t(|key, _| lag_core::i18n::tr(&key).into());
    ui.set_theme_name(theme.name.clone().into());
    let st = Rc::new(RefCell::new(State {
        dir,
        theme,
        instance: Rc::default(),
        raw: None,
        sources: Vec::new(),
        selected: None,
        page: Rc::new(std::cell::Cell::new(1)),
        filter: String::new(),
        undo: Vec::new(),
        redo: Vec::new(),
    }));
    if let Err(e) = load(&ui, &mut st.borrow_mut()) {
        report(&ui, Err(e));
    }

    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_pick(move |x, y| {
        let Some(ui) = weak.upgrade() else { return };
        let picked = pick(&s.borrow(), x, y);
        s.borrow_mut().selected = picked;
        refresh(&ui, &s.borrow());
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_select_crumb(move |i| {
        let Some(ui) = weak.upgrade() else { return };
        let chosen = s.borrow().selected.as_ref().and_then(|sel| crumbs(sel).into_iter().nth(i as usize));
        if chosen.is_some() {
            s.borrow_mut().selected = chosen;
        }
        refresh(&ui, &s.borrow());
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_filter_changed(move |f| {
        let Some(ui) = weak.upgrade() else { return };
        s.borrow_mut().filter = f.to_string();
        refresh(&ui, &s.borrow());
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_set_prop(move |name, input| {
        let Some(ui) = weak.upgrade() else { return };
        edit_selected(&ui, &s, |st, sel, text| {
            let kind = props_of(st, sel).iter().find(|p| p.name == name).map_or(0, |p| p.kind);
            let value = expression(kind, &input);
            let after = set_binding(text, &sel.path, sel.offset, &name, &value)?;
            Ok((after, format!("{}: {name} = {value}", sel.label)))
        });
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_reset_prop(move |name| {
        let Some(ui) = weak.upgrade() else { return };
        edit_selected(&ui, &s, |_, sel, text| {
            let after = remove_binding(text, &sel.path, sel.offset, &name)?;
            Ok((after, format!("{}: {name} = default", sel.label)))
        });
    });
    for redo in [false, true] {
        let (s, weak) = (st.clone(), ui.as_weak());
        let go = move || {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut st = s.borrow_mut();
                let edit = if redo { st.redo.pop() } else { st.undo.pop() };
                let Some(edit) = edit else { return };
                let text = if redo { edit.after.clone() } else { edit.before.clone() };
                let old = st.selected.clone();
                let _ = change(&ui, &mut st, &edit.path, text, false);
                if redo {
                    st.undo.push(edit)
                } else {
                    st.redo.push(edit)
                }
                if let Some(old) = old {
                    reselect(&mut st, &old);
                }
            }
            refresh_soon(&ui, &s);
        };
        if redo {
            ui.on_redo(go);
        } else {
            ui.on_undo(go);
        }
    }
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_structure(move |op| {
        let Some(ui) = weak.upgrade() else { return };
        let result = {
            let mut st = s.borrow_mut();
            let Some(sel) = st.selected.clone() else { return };
            let r = std::fs::read_to_string(&sel.path)
                .map_err(|e| e.to_string())
                .and_then(|t| restructure(&t, &sel.path, sel.offset, &op))
                .and_then(|(after, next)| change(&ui, &mut st, &sel.path, after, true).map(|_| next));
            match r {
                Ok(next) => {
                    let parent = crumbs(&sel).into_iter().rev().nth(1);
                    st.selected = match next {
                        Some(at) => st.sources.iter().find(|x| x.path == sel.path && x.offset == at).cloned(),
                        None => parent.and_then(|p| st.sources.iter().find(|x| x.path == p.path && x.label == p.label).cloned()),
                    };
                    Ok(format!("{}: {op}", sel.label))
                }
                Err(e) => {
                    reselect(&mut st, &sel);
                    Err(e)
                }
            }
        };
        report(&ui, result);
        refresh_soon(&ui, &s);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_drag_end(move |dx, dy| {
        let Some(ui) = weak.upgrade() else { return };
        let target = {
            let st = s.borrow();
            let (Some(sel), Some(inst)) = (st.selected.clone(), live(&st)) else { return };
            let text = std::fs::read_to_string(&sel.path).unwrap_or_default();
            let layout = find_element(&parse_text(&text, &sel.path), sel.offset).is_some_and(|e| in_layout(&e));
            let first = inst.component_positions(&sel.path, sel.offset).into_iter().next();
            (layout, first.map(|h| (h.rect.origin.x - h.parent_origin().x, h.rect.origin.y - h.parent_origin().y)))
        };
        match target {
            (true, _) => report(&ui, Err(lag_core::i18n::tr("ThemeEditor_InLayout").to_owned())),
            (false, Some((x, y))) => edit_selected(&ui, &s, |_, sel, text| {
                let (nx, ny) = ((x + dx).round(), (y + dy).round());
                let t = set_binding(text, &sel.path, sel.offset, "x", &format!("{nx}px"))?;
                let t = set_binding(&t, &sel.path, sel.offset, "y", &format!("{ny}px"))?;
                Ok((t, format!("{}: x = {nx}px, y = {ny}px", sel.label)))
            }),
            _ => {}
        }
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_page(move |p| {
        let Some(ui) = weak.upgrade() else { return };
        s.borrow().page.set(p);
        if let Some(i) = live(&s.borrow()) {
            let _ = i.set_global_property("MainVM", "current-page", Value::EnumerationValue("Page".into(), PAGES[p.clamp(0, 3) as usize].into()));
        }
        refresh_soon(&ui, &s);
    });
    ui.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    if ui.show().is_err() {
        return;
    }
    script(&ui);
    let _ = slint::run_event_loop_until_quit();
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}

/// `--editor-script <file>`: commands run one by one (checks without the mouse): `pick X Y`, `set NAME VALUE`,
/// `reset NAME`, `crumb N`, `page N`, `undo`, `redo`, `filter TEXT`, `wait MS`, `quit`.
fn script(ui: &ThemeEditorWindow) {
    let a: Vec<String> = std::env::args().collect();
    let Some(file) = a.iter().position(|x| x == "--editor-script").and_then(|i| a.get(i + 1)) else { return };
    let lines: Vec<String> = std::fs::read_to_string(file).unwrap_or_default().lines().map(str::to_owned).collect();
    fn step(weak: crate::ui_twin::Weak<ThemeEditorWindow>, lines: Rc<Vec<String>>, i: usize) {
        let Some(line) = lines.get(i) else { return };
        let Some(ui) = weak.upgrade() else { return };
        let line = line.trim();
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let mut delay = 700u64;
        match cmd {
            "pick" => {
                let n: Vec<f32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                ui.invoke_pick(n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0));
            }
            "set" => {
                let (n, v) = rest.split_once(' ').unwrap_or((rest, ""));
                ui.invoke_set_prop(n.into(), v.into());
            }
            "reset" => ui.invoke_reset_prop(rest.into()),
            "do" => ui.invoke_structure(rest.trim().into()),
            "drag" => {
                let n: Vec<f32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
                ui.invoke_drag_end(n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0));
            }
            "crumb" => ui.invoke_select_crumb(rest.trim().parse().unwrap_or(0)),
            "page" => ui.invoke_page(rest.trim().parse().unwrap_or(1)),
            "undo" => ui.invoke_undo(),
            "redo" => ui.invoke_redo(),
            "filter" => ui.invoke_filter_changed(rest.into()),
            "wait" => delay = rest.trim().parse().unwrap_or(500),
            "quit" => {
                let _ = slint::quit_event_loop();
                return;
            }
            _ => delay = 0,
        }
        slint::Timer::single_shot(Duration::from_millis(delay), move || step(weak, lines, i + 1));
    }
    let (weak, lines) = (ui.as_weak(), Rc::new(lines));
    slint::Timer::single_shot(Duration::from_millis(1500), move || step(weak, lines, 0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_replaced_added_and_removed() {
        let p = Path::new("t.slint");
        let text = "export component A inherits Window {\n    r := Rectangle {\n        background: red;\n    }\n    Text { }\n}\n";
        let rect_at = text.find("Rectangle").unwrap() as u32;
        let out = set_binding(text, p, rect_at, "background", "#3DDCFF").unwrap();
        assert!(out.contains("background: #3DDCFF;") && !out.contains("red"), "{out}");
        let out = set_binding(&out, p, rect_at, "border-radius", "12px").unwrap();
        assert!(out.contains("r := Rectangle {\n        border-radius: 12px;"), "{out}");
        let text_at = out.find("Text").unwrap() as u32;
        let out = set_binding(&out, p, text_at, "color", "white").unwrap();
        assert!(out.contains("Text {\n        color: white;"), "{out}");
        assert!(set_binding(&out, p, 3, "x", "1px").is_err());
        let rect_at = out.find("Rectangle").unwrap() as u32;
        let back = remove_binding(&out, p, rect_at, "border-radius").unwrap();
        assert!(!back.contains("border-radius") && back.contains("r := Rectangle {\n        background: #3DDCFF;"), "{back}");
        let root = parse_text(&back, p);
        assert_eq!(binding_text(&find_element(&root, rect_at).unwrap(), "background").as_deref(), Some("#3DDCFF"));
    }

    #[test]
    fn elements_are_removed_copied_reordered_and_added() {
        let p = Path::new("t.slint");
        let text = "export component W inherits Window {\n    HorizontalLayout {\n        a := Rectangle { }\n        Text { text: \"b\"; }\n    }\n}\n";
        let a_at = text.find("Rectangle").unwrap() as u32;
        let b_at = text.find("Text").unwrap() as u32;
        // down: a goes after b, and its new place is returned
        let (moved, at) = restructure(text, p, a_at, "down").unwrap();
        assert!(moved.find("Text").unwrap() < moved.find("Rectangle").unwrap(), "{moved}");
        assert_eq!(&moved[at.unwrap() as usize..at.unwrap() as usize + 9], "Rectangle");
        let (back, _) = restructure(&moved, p, at.unwrap(), "up").unwrap();
        assert_eq!(back, text);
        assert!(restructure(text, p, a_at, "up").is_err(), "already first");
        // the ids inside a copy are renamed, with the references to them
        let inner = "export component W inherits Window {\n    Rectangle {\n        background: t.pressed ? red : blue;\n        t := TouchArea { }\n    }\n}\n";
        let (dup2, _) = restructure(inner, p, inner.find("Rectangle").unwrap() as u32, "duplicate").unwrap();
        assert!(dup2.contains("t-2 := TouchArea") && dup2.contains("t-2.pressed") && dup2.matches("t := TouchArea").count() == 1, "{dup2}");
        // duplicate without the id
        let (dup, at) = restructure(text, p, a_at, "duplicate").unwrap();
        assert_eq!(dup.matches("Rectangle { }").count(), 2);
        assert_eq!(dup.matches("a :=").count(), 1);
        assert_eq!(&dup[at.unwrap() as usize..at.unwrap() as usize + 9], "Rectangle");
        // delete takes the whole line
        let (del, next) = restructure(text, p, b_at, "delete").unwrap();
        assert!(!del.contains("Text") && del.contains("a := Rectangle { }\n    }"), "{del}");
        assert_eq!(next, None);
        // the window's root stays
        assert!(restructure(text, p, text.find("Window").unwrap() as u32, "delete").is_err());
        // add a rectangle inside the layout, on its own line, `}` kept on its line
        let lay_at = text.find("HorizontalLayout").unwrap() as u32;
        let (added, at) = restructure(text, p, lay_at, "add-rect").unwrap();
        assert!(added.contains("Text { text: \"b\"; }\n        Rectangle { x: 24px;"), "{added}");
        assert!(added.contains("border-radius: 14px; }\n    }\n}"), "{added}");
        assert_eq!(&added[at.unwrap() as usize..at.unwrap() as usize + 9], "Rectangle");
        let root = parse_text(text, p);
        assert!(in_layout(&find_element(&root, a_at).unwrap()));
        assert!(!in_layout(&find_element(&root, lay_at).unwrap()));
        assert_eq!(label_of(&find_element(&root, text.find("Window").unwrap() as u32).unwrap()), "W");
    }

    #[test]
    fn typed_input_becomes_an_expression() {
        assert_eq!(expression(2, "12"), "12px");
        assert_eq!(expression(2, "50%"), "50%");
        assert_eq!(expression(5, "Hello \"you\""), "\"Hello \\\"you\\\"\"");
        assert_eq!(expression(5, "Tr.t(\"Btn_SaveReplay\", Tr.rev)"), "Tr.t(\"Btn_SaveReplay\", Tr.rev)");
        assert_eq!(expression(7, "art/bg.png"), "@image-url(\"art/bg.png\")");
        assert_eq!(expression(1, "#FF4FA3"), "#FF4FA3");
        assert_eq!(parse_color("#3DF"), Some(Color::from_rgb_u8(0x33, 0xDD, 0xFF)));
    }
}
