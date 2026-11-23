//! Theme editor (docs/spec/themes.md, phase 4): `Lag.exe --theme-editor <theme folder>`. The theme runs live inside
//! the editor window (a component factory); a click picks the element under the pointer (Slint's own source-to-item
//! mapping: every element of the theme's files is located on screen); the panel lists every property the element
//! has (from the compiler's type of it) with what the file says now; a change is written into the theme's `.slint`
//! file as a text edit, compiled, and swapped in. An edit that does not compile is undone in the file at once and
//! the compiler's message is shown. Undo / redo keep whole-file texts.
//!
//! The editor shows the theme put together over Lag's own UI (`themes::sync`): every element is there, the
//! theme's or Lag's. The first change in a file of Lag's copies it into the theme; a file that becomes the same
//! as Lag's again leaves the theme (it inherits Lag's, newer versions included).
use crate::{app_ctx, startup, themes, ui_twin, views, AppWindow, ComboBoxItemData, ComponentHandle, EditorCrumb, EditorProp, EditorRect, EditorToken, ThemeEditorWindow, Tr};
use i_slint_compiler::langtype::Type;
use i_slint_compiler::object_tree::ElementRc;
use i_slint_compiler::parser::{SyntaxKind, SyntaxNode};
use slint::{Color, ComponentFactory, ModelRc, VecModel};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use lag_core::i18n::tr;
use std::time::Duration;

mod palette;

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
    let el = find_element(&root, offset).ok_or(tr("ThemeEditor_ErrMoved"))?;
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
    let el = find_element(&root, offset).ok_or(tr("ThemeEditor_ErrMoved"))?;
    let b = binding_node(&el, name).ok_or(tr("ThemeEditor_ErrNotSet"))?;
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
    let el = find_element(&root, offset).ok_or(tr("ThemeEditor_ErrMoved"))?;
    let mut out = text.to_owned();
    match op {
        "delete" => {
            let b = block_of(&el).ok_or(tr("ThemeEditor_ErrRoot"))?;
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
            let b = block_of(&el).ok_or(tr("ThemeEditor_ErrRoot"))?;
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
            let b = block_of(&el).ok_or(tr("ThemeEditor_ErrRoot"))?;
            let parent = b.parent().ok_or("no parent")?;
            let siblings: Vec<SyntaxNode> = parent.children().filter(is_block).collect();
            let i = siblings.iter().position(|n| n.text_range() == b.text_range()).ok_or("not found")?;
            let j = if op == "up" { i.checked_sub(1) } else { (i + 1 < siblings.len()).then_some(i + 1) };
            let j = j.ok_or(tr(if op == "up" { "ThemeEditor_ErrFirst" } else { "ThemeEditor_ErrLast" }))?;
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

/// One change of one of the theme's own files (None: the theme has no such file, Lag's is used).
struct Edit {
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

struct State {
    dir: PathBuf,
    /// the theme put together over Lag's UI: what is compiled and shown (`themes::sync`)
    build: PathBuf,
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
    /// the theme's colours (palette tab)
    tokens: Vec<palette::Token>,
    /// the colour mode on the canvas, which the palette edits ("dark" | "light" | "neon")
    mode: Rc<RefCell<String>>,
    all_modes: bool,
    token_filter: String,
    /// what the colour picker changes
    picker: Option<Target>,
    /// pictures this editor copied into the theme (removed at the end when nothing names them any more)
    imported: Vec<PathBuf>,
}

#[derive(Clone)]
enum Target {
    /// a property of the selected element
    Prop(String),
    /// a palette colour (index into `tokens`)
    Token(usize),
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

/// The API globals the theme reads (what Lag shows: clips, settings, the player...).
const DATA_GLOBALS: &[&str] = &["Tr", "MainVM", "LibraryVM", "PlayerVM", "SettingsVM", "EditorVM"];

thread_local! {
    /// A hidden Lag window driven by Lag's own code (library scan, settings, player), never shown and never
    /// saving: the theme on the canvas shows its data, so there are real clip cards, settings rows... to edit.
    static DONOR: RefCell<Option<(AppWindow, Rc<app_ctx::AppCtx>)>> = const { RefCell::new(None) };
}

fn start_donor(settings: &lag_core::settings::SettingsData) {
    let Ok(ui) = AppWindow::new() else { return };
    ui.global::<Tr>().on_t(|key, _| lag_core::i18n::tr(&key).into());
    let mut s = settings.clone();
    s.auto_start_recording = false;
    s.start_minimized = false;
    s.stats_shown = false;
    let ctx = app_ctx::AppCtx::new(s);
    ctx.never_save();
    views::init_all(&ui, &ctx);
    DONOR.with(|d| *d.borrow_mut() = Some((ui, ctx)));
}

/// Copies Lag's live data into the theme on the canvas (the page stays the editor's choice).
fn mirror(to: &ComponentInstance) {
    DONOR.with(|d| {
        let d = d.borrow();
        let Some((donor, _)) = d.as_ref() else { return };
        let from = donor.instance();
        let def = slint_interpreter::ComponentHandle::clone_strong(from).definition();
        for g in DATA_GLOBALS {
            let Some(props) = def.global_properties(g) else { continue };
            for (name, _) in props {
                if *g == "MainVM" && name == "current-page" {
                    continue;
                }
                if let Ok(v) = from.get_global_property(g, &name) {
                    if to.get_global_property(g, &name).ok().as_ref() != Some(&v) {
                        let _ = to.set_global_property(g, &name, v);
                    }
                }
            }
        }
    });
}

fn factory(
    def: ComponentDefinition,
    slot: Rc<RefCell<Option<ComponentInstance>>>,
    page: Rc<std::cell::Cell<i32>>,
    mode: Rc<RefCell<String>>,
) -> Value {
    Value::ComponentFactory(ComponentFactory::new(move |ctx| {
        let inst = def.create_embedded(ctx).ok()?;
        fill(&inst, page.get());
        // a theme without Lag's colour modes has no such property: nothing to set
        let _ = inst.set_property("theme-mode", Value::String(mode.borrow().as_str().into()));
        mirror(&inst);
        *slot.borrow_mut() = Some(slint::ComponentHandle::clone_strong(&inst));
        Some(inst)
    }))
}

/// Compiles the theme and puts it on the canvas. Err = the compiler's text.
fn load(ui: &ThemeEditorWindow, st: &mut State) -> Result<(), String> {
    let (defs, _) = themes::compile_theme(&st.theme)?;
    let def = defs.into_iter().find(|d| d.name() == "AppWindow").ok_or("the theme has no AppWindow")?;
    st.raw = def.raw_type_loader();
    st.build = themes::build_dir(&st.theme);
    st.sources = sources(&st.build);
    st.tokens = palette::tokens(&st.build);
    let slot = factory(def, st.instance.clone(), st.page.clone(), st.mode.clone());
    ui.instance().set_property("canvas", slot).map_err(|e| format!("{e:?}"))?;
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

/// Every element of the theme's files under (x, y), innermost first (one pass over what is on screen; elements
/// from Lag's own files, reached through `@lag/`, are not the theme's and are left out).
fn hits(st: &State, x: f32, y: f32) -> Vec<Source> {
    let Some(inst) = live(st) else { return Vec::new() };
    let mut found: Vec<(f32, bool, Source)> = Vec::new();
    for (path, offset, rect, use_site) in inst.element_sources_at(x, y) {
        let Some(s) = st.sources.iter().find(|s| s.offset == offset && s.path == path) else { continue };
        let area = rect.rect.size.width * rect.rect.size.height;
        match found.iter_mut().find(|(_, _, f)| f == s) {
            Some(f) if area < f.0 => f.0 = area,
            Some(_) => {}
            None => found.push((area, use_site, s.clone())),
        }
    }
    // innermost first; for the same size, the use of a component (this button) before its body (every button),
    // then what is drawn on top (written later: a child after its parent, a sibling after the ones before it)
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)).then(b.2.offset.cmp(&a.2.offset)));
    found.into_iter().map(|(_, _, s)| s).collect()
}

/// The innermost element under (x, y); a second click on the same spot picks the one around it.
fn pick(st: &State, x: f32, y: f32) -> Option<Source> {
    let started = std::time::Instant::now();
    let hits = hits(st, x, y);
    log::debug!("[ThemeEditor] pick {x},{y}: {} hits in {} ms", hits.len(), started.elapsed().as_millis());
    if std::env::var_os("LAG_EDITOR_DEBUG").is_some() {
        log::info!("[ThemeEditor] pick {x},{y}: {} hits in {} ms", hits.len(), started.elapsed().as_millis());
    }
    let current = st.selected.as_ref().and_then(|c| hits.iter().position(|s| s == c));
    let i = current.map_or(0, |i| (i + 1) % hits.len().max(1));
    hits.into_iter().nth(i)
}

/// The component an element is written in, when a change to it shows in several places: the component is used
/// more than once, or in a `for` (QA 2026-11-22: "changes every SettingsPage" for a page that exists once).
fn template_of(st: &State, s: &Source) -> Option<String> {
    let text = std::fs::read_to_string(&s.path).ok()?;
    let root = parse_text(&text, &s.path);
    let mut n = find_element(&root, s.offset)?.parent();
    let name = loop {
        let p = n?;
        if p.kind() == SyntaxKind::Component {
            let name = p.child_node(SyntaxKind::DeclaredIdentifier)?.text().to_string().trim().to_owned();
            let window = p
                .child_node(SyntaxKind::Element)
                .and_then(|e| e.child_node(SyntaxKind::QualifiedName))
                .is_some_and(|q| q.text().to_string().trim() == "Window");
            if window {
                return None;
            }
            break name;
        }
        n = p.parent();
    };
    // the uses of the component (its own root carries its name too: not a use)
    let node_of = |u: &Source| {
        let t = std::fs::read_to_string(&u.path).ok()?;
        find_element(&parse_text(&t, &u.path), u.offset)
    };
    let uses: Vec<&Source> = st
        .sources
        .iter()
        .filter(|x| x.label.rsplit(" := ").next() == Some(name.as_str()))
        .filter(|x| node_of(x).is_some_and(|e| e.parent().is_none_or(|p| p.kind() != SyntaxKind::Component)))
        .collect();
    let repeated = |u: &Source| {
        let mut at = node_of(u).and_then(|e| e.parent());
        while let Some(p) = at {
            match p.kind() {
                SyntaxKind::RepeatedElement => return true,
                SyntaxKind::Component => return false,
                _ => at = p.parent(),
            }
        }
        false
    };
    (uses.len() > 1 || uses.first().is_some_and(|u| repeated(u))).then_some(name)
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
    // x / y / width / height are every element's, but the builtin types do not list them (QA 2026-11-22: a new
    // shape could not be placed or sized by number)
    for g in ["x", "y", "width", "height"] {
        if !list.iter().any(|(n, _)| n == g) {
            list.push((g.into(), Type::LogicalLength));
        }
    }
    // properties the element declares itself (`in property <..> x`)
    list.extend(el.borrow().property_declarations.iter().map(|(k, d)| (k.clone(), d.property_type.clone())));
    let filter = st.filter.trim().to_lowercase();
    let mut props: Vec<EditorProp> = list
        .into_iter()
        .filter(|(n, _)| !n.starts_with("accessible-") && (filter.is_empty() || n.to_lowercase().contains(&filter)))
        .filter_map(|(name, t)| {
            let (kind, label, options) = kind_of(&t)?;
            let value = node.as_ref().and_then(|n| binding_text(n, &name)).unwrap_or_default();
            // a literal colour, or a colour of the theme's palette (`Theme.on-accent`) as the chosen mode shows it
            let known = parse_color(&value).or_else(|| palette::colour_named(&value, &st.tokens, &st.mode.borrow()));
            let swatch = known.unwrap_or(Color::from_argb_u8(0, 0, 0, 0));
            // `ImageFit.cover` or `cover`: which of the values
            let short = value.rsplit('.').next().unwrap_or(&value).trim();
            let option_index = options.iter().position(|o| o == short).map_or(-1, |i| i as i32);
            let picture = if kind == 7 { picture_of(&s.path, &value).unwrap_or_default() } else { Default::default() };
            Some(EditorProp {
                name: name.to_string().into(),
                kind,
                type_label: label.into(),
                bound: !value.is_empty(),
                value: value.into(),
                options: ModelRc::new(VecModel::from(
                    options.into_iter().map(|o| ComboBoxItemData { text: o.into(), tier: 0 }).collect::<Vec<_>>(),
                )),
                option_index,
                swatch,
                known: known.is_some(),
                picture,
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

/// The picture `@image-url("art/bg.png")` names, read relative to the `.slint` file that names it.
fn picture_of(slint_file: &Path, value: &str) -> Option<slint::Image> {
    let rel = value.trim().strip_prefix("@image-url(")?.trim_start().strip_prefix('"')?;
    let rel = &rel[..rel.find('"')?];
    slint::Image::load_from_path(&slint_file.parent()?.join(rel)).ok()
}

/// `#RRGGBB`, or `#RRGGBBAA` when the colour is not opaque.
fn hex(c: Color) -> String {
    if c.alpha() == 255 {
        format!("#{:02X}{:02X}{:02X}", c.red(), c.green(), c.blue())
    } else {
        format!("#{:02X}{:02X}{:02X}{:02X}", c.red(), c.green(), c.blue(), c.alpha())
    }
}

/// Brings a picture into the theme: a file of the theme (or of the built theme: one of Lag's) stays where it is,
/// anything else is copied into the theme's `art/` (the same file is not copied twice; another file of the same
/// name gets `-2`...). Returns the path to write in `@image-url(..)` in `slint_file` (a file of the built theme
/// `build`, where the theme's files land at the same relative paths).
fn import_picture(theme_dir: &Path, build: &Path, file: &Path, slint_file: &Path) -> Result<String, String> {
    let clean = i_slint_compiler::pathutils::clean_path;
    let (dir, build, file) = (clean(theme_dir), clean(build), clean(file));
    if file.starts_with(&build) {
        return Ok(relative(&clean(slint_file.parent().unwrap_or(&build)), &file));
    }
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !["png", "jpg", "jpeg", "svg", "gif", "webp"].contains(&ext.as_str()) {
        return Err(lag_core::i18n::format("ThemeEditor_ErrNotPicture", &[&file.display().to_string()]).unwrap_or_default());
    }
    let target = if file.starts_with(&dir) {
        file
    } else {
        let art = dir.join("art");
        std::fs::create_dir_all(&art).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "picture".into());
        let mut n = 1;
        loop {
            let name = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") };
            let at = art.join(name);
            match std::fs::read(&at) {
                Ok(existing) if existing == bytes => break at,
                Ok(_) => n += 1,
                Err(_) => {
                    std::fs::write(&at, &bytes).map_err(|e| e.to_string())?;
                    break at;
                }
            }
        }
    };
    let in_build = build.join(target.strip_prefix(&dir).map_err(|e| e.to_string())?);
    Ok(relative(&clean(slint_file.parent().unwrap_or(&build)), &in_build))
}

/// `to` written from `from` (a folder): `../art/bg.png`, with forward slashes.
fn relative(from: &Path, to: &Path) -> String {
    let (a, b): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let same = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - same];
    parts.extend(b[same..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
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

/// The theme's own file for a file of the built theme.
fn own_path(st: &State, file: &Path) -> Result<PathBuf, String> {
    let rel = file.strip_prefix(&st.build).map_err(|_| format!("{} is not part of the theme", file.display()))?;
    Ok(st.dir.join(rel))
}

/// The same code, whatever the spaces and line breaks between its tokens (a shape added to a one-line element
/// and removed again leaves the `}` on a line of its own: still Lag's file).
fn same_code(a: &str, b: &str) -> bool {
    let tokens = |t: &str| {
        i_slint_compiler::lexer::lex(t)
            .into_iter()
            .filter(|k| k.kind != SyntaxKind::Whitespace)
            .map(|k| (k.kind, k.text))
            .collect::<Vec<_>>()
    };
    a == b || tokens(a) == tokens(b)
}

/// Writes the theme's own `target` (None: removes it). A text the same as Lag's own file (as code) is not kept:
/// the theme inherits Lag's (and its later versions).
fn write_own(st: &State, target: &Path, text: Option<&str>) -> Result<(), String> {
    let lag = target.strip_prefix(&st.dir).ok().map(|rel| ui_twin::builtin_dir().join(rel));
    let lag_text = lag.as_ref().and_then(|l| std::fs::read_to_string(l).ok());
    let same_as_lag = text.is_some_and(|t| lag_text.as_deref().is_some_and(|l| same_code(t, l)));
    match text {
        Some(t) if !same_as_lag => {
            std::fs::create_dir_all(target.parent().unwrap_or(&st.dir)).map_err(|e| e.to_string())?;
            std::fs::write(target, t).map_err(|e| e.to_string())
        }
        _ => {
            match std::fs::remove_file(target) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            }
            // folders left empty go too (only empty ones: remove_dir refuses the others)
            let mut at = target.parent();
            while let Some(d) = at.filter(|d| d.starts_with(&st.dir) && *d != st.dir.as_path()) {
                if std::fs::remove_dir(d).is_err() {
                    break;
                }
                at = d.parent();
            }
            Ok(())
        }
    }
}

/// Puts `after` in place of the theme's own `target` and compiles; an error puts `before` back and returns the
/// compiler's message.
fn apply(ui: &ThemeEditorWindow, st: &mut State, target: &Path, before: Option<String>, after: Option<String>, record: bool) -> Result<(), String> {
    write_own(st, target, after.as_deref())?;
    if let Err(e) = load(ui, st) {
        let _ = write_own(st, target, before.as_deref());
        let _ = load(ui, st);
        return Err(lag_core::i18n::format("ThemeEditor_NotApplied", &[&e]).unwrap_or(e));
    }
    // a .lag theme: the change goes into its one file at once
    themes::pack(&st.theme)?;
    if record {
        st.undo.push(Edit { path: target.to_path_buf(), before, after });
        st.redo.clear();
    }
    Ok(())
}

/// A file of the built theme (`file`, the theme's or Lag's) as `after`: written as the theme's own, compiled; an
/// error puts the theme back and returns the compiler's message.
fn change(ui: &ThemeEditorWindow, st: &mut State, file: &Path, after: String, record: bool) -> Result<(), String> {
    let shown = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    if shown == after {
        return Ok(());
    }
    let target = own_path(st, file)?;
    let before = std::fs::read_to_string(&target).ok();
    apply(ui, st, &target, before, Some(after), record)
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

/// The palette tab: the theme's colours as the chosen mode shows them, a section title on the first of each.
fn palette_rows(st: &State) -> Vec<EditorToken> {
    let mode = st.mode.borrow().clone();
    let f = st.token_filter.trim().to_lowercase();
    let mut last = None;
    st.tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| f.is_empty() || t.name.to_lowercase().contains(&f) || t.section.to_lowercase().contains(&f))
        .map(|(i, t)| {
            let value = match palette::active_branch(&t.expr, &mode) {
                Some((a, b)) if !st.all_modes => t.expr[a..b].trim().to_owned(),
                _ => t.expr.clone(),
            };
            let group = (t.global.as_str(), t.section.as_str());
            let header = if last != Some(group) {
                last = Some(group);
                if t.section.is_empty() { t.global.clone() } else { t.section.clone() }
            } else {
                String::new()
            };
            EditorToken {
                header: header.into(),
                name: t.name.clone().into(),
                value: value.into(),
                swatch: palette::colour_of(t, &st.tokens, &mode).unwrap_or(Color::from_argb_u8(0, 0, 0, 0)),
                index: i as i32,
                known: palette::colour_of(t, &st.tokens, &mode).is_some(),
            }
        })
        .collect()
}

fn refresh(ui: &ThemeEditorWindow, st: &State) {
    ui.set_tokens(ModelRc::new(VecModel::from(palette_rows(st))));
    ui.set_has_modes(st.tokens.iter().any(|t| t.expr.contains("is-dark") || t.expr.contains("is-neon")));
    ui.set_palette_mode(st.mode.borrow().as_str().into());
    ui.set_all_modes(st.all_modes);
    let inst = live(st);
    match (&st.selected, inst) {
        (Some(s), Some(i)) => {
            let dbg = std::env::var_os("LAG_EDITOR_DEBUG").is_some();
            let t0 = std::time::Instant::now();
            let lap = |what: &str| {
                if dbg {
                    log::info!("[ThemeEditor] refresh {what}: {} ms", t0.elapsed().as_millis());
                }
            };
            let rel_path = s.path.strip_prefix(&st.build).unwrap_or(&s.path);
            let rel = rel_path.display().to_string();
            // still Lag's own file: the first change copies it into the theme
            let inherited = if st.dir.join(rel_path).is_file() {
                String::new()
            } else {
                format!("\n{}", lag_core::i18n::tr("ThemeEditor_Inherited"))
            };
            ui.set_selection(ModelRc::new(VecModel::from(rects(&i, s))));
            lap("rects");
            ui.set_selected_name(s.label.clone().into());
            let template = template_of(st, s)
                .map(|c| format!("\n{}", lag_core::i18n::format("ThemeEditor_Template", &[&c]).unwrap_or(c)))
                .unwrap_or_default();
            ui.set_selected_where(format!("{rel}:{}{inherited}{template}", s.line).into());
            lap("template");
            let chain = crumbs(s);
            let crumbs: Vec<EditorCrumb> = chain
                .iter()
                .enumerate()
                .map(|(i, c)| EditorCrumb { label: c.label.clone().into(), index: i as i32, current: c == s })
                .collect();
            ui.set_crumbs(ModelRc::new(VecModel::from(crumbs)));
            lap("crumbs");
            ui.set_props(ModelRc::new(VecModel::from(props_of(st, s))));
            lap("props");
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

/// The pictures this editor brought in that no file of the theme names any more (a change undone): removed when
/// the editor closes (until then Redo may need them).
fn remove_unused_pictures(st: &State) {
    if st.imported.is_empty() {
        return;
    }
    let mut texts = String::new();
    let mut stack = vec![st.dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "slint") {
                texts.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
            }
        }
    }
    for pic in &st.imported {
        let name = pic.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !name.is_empty() && !texts.contains(&name) && std::fs::remove_file(pic).is_ok() {
            log::info!("[ThemeEditor] removed the unused picture {name}");
        }
    }
    // an art/ left empty goes too
    let _ = std::fs::remove_dir(st.dir.join("art"));
}

/// `--editor-shots <out-dir>`: the editor runs in memory, the `--editor-script` runs to its end, `shot NAME` saves
/// `<out-dir>/NAME.png`.
fn shots_dir() -> Option<PathBuf> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == "--editor-shots").and_then(|i| a.get(i + 1)).map(PathBuf::from)
}

fn set_prop(ui: &ThemeEditorWindow, s: &Rc<RefCell<State>>, name: &str, input: &str) {
    edit_selected(ui, s, |st, sel, text| {
        let kind = props_of(st, sel).iter().find(|p| p.name == name).map_or(0, |p| p.kind);
        let value = expression(kind, input);
        let after = set_binding(text, &sel.path, sel.offset, name, &value)?;
        Ok((after, format!("{}: {name} = {value}", sel.label)))
    });
}

fn set_token(ui: &ThemeEditorWindow, s: &Rc<RefCell<State>>, index: usize, input: &str) {
    let result = {
        let mut st = s.borrow_mut();
        let Some(t) = st.tokens.get(index).cloned() else { return };
        let (mode, all) = (st.mode.borrow().clone(), st.all_modes);
        let value = palette::colour_input(input);
        std::fs::read_to_string(&t.path)
            .map_err(|e| e.to_string())
            .and_then(|text| palette::set_token(&text, &t.path, t.offset, &mode, all, &value))
            .and_then(|after| change(ui, &mut st, &t.path, after, true))
            .map(|_| format!("{}.{} = {value}", t.global, t.name))
    };
    report(ui, result);
    refresh_soon(ui, s);
}

/// Shows the colour picker on `colour` for what `State::picker` names.
fn open_picker(ui: &ThemeEditorWindow, st: &State, colour: Color, title: &str) {
    let hsva = colour.to_hsva();
    ui.set_picker_hue(hsva.hue.rem_euclid(360.0));
    ui.set_picker_sat(hsva.saturation);
    ui.set_picker_val(hsva.value);
    ui.set_picker_alpha(hsva.alpha);
    ui.set_picker_old(colour);
    ui.set_picker_title(title.into());
    // the theme's colours, as the chosen mode shows them (all of them, whatever the palette tab filters)
    let mode = st.mode.borrow().clone();
    let swatches: Vec<EditorToken> = st
        .tokens
        .iter()
        .enumerate()
        .map(|(i, t)| EditorToken {
            header: Default::default(),
            name: format!("{}.{}", t.global, t.name).into(),
            value: Default::default(),
            swatch: palette::colour_of(t, &st.tokens, &mode).unwrap_or(Color::from_argb_u8(0, 0, 0, 0)),
            index: i as i32,
            known: palette::colour_of(t, &st.tokens, &mode).is_some(),
        })
        .collect();
    ui.set_picker_swatches(ModelRc::new(VecModel::from(swatches)));
    ui.set_picker_open(true);
}

/// Writes what the picker chose (`#RRGGBB`, a typed colour, a palette name) into its target.
fn apply_picked(ui: &ThemeEditorWindow, s: &Rc<RefCell<State>>, value: &str) {
    let target = s.borrow().picker.clone();
    match target {
        Some(Target::Prop(name)) => set_prop(ui, s, &name, value),
        Some(Target::Token(i)) => set_token(ui, s, i, value),
        None => {}
    }
}

/// A picture file chosen for the selected element's `name`: brought into the theme and named there.
fn set_picture(ui: &ThemeEditorWindow, s: &Rc<RefCell<State>>, name: &str, file: &Path) {
    let rel = {
        let mut st = s.borrow_mut();
        let Some(sel) = st.selected.clone() else { return };
        let art = st.dir.join("art");
        let before: Vec<PathBuf> = std::fs::read_dir(&art).map(|e| e.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        let rel = import_picture(&st.dir, &st.build, file, &sel.path);
        let new: Vec<PathBuf> = std::fs::read_dir(&art)
            .map(|e| e.flatten().map(|e| e.path()).filter(|p| !before.contains(p)).collect())
            .unwrap_or_default();
        st.imported.extend(new);
        rel
    };
    match rel {
        Ok(rel) => set_prop(ui, s, name, &format!("@image-url(\"{rel}\")")),
        Err(e) => report(ui, Err(e)),
    }
}

pub fn run(dir: PathBuf) {
    if log::set_logger(&crate::STDERR_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    // before any window exists
    let headless = shots_dir().and_then(|_| crate::snapshot::install_for_editor(1640, 960));
    let settings = startup::load_settings();
    lag_core::i18n::set_language(settings.language.as_deref().unwrap_or("en"));
    if let Err(e) = ui_twin::load_builtin() {
        startup::fatal_message(&format!("Lag could not load its interface:\n\n{e}"));
        return;
    }
    let theme = match themes::read(&dir).and_then(|t| themes::materialize(&t).map(|_| t)) {
        Ok(t) => t,
        Err(e) => {
            startup::fatal_message(&format!("Not a Lag theme folder:\n\n{e}"));
            return;
        }
    };
    let Ok(ui) = ThemeEditorWindow::new() else { return };
    ui.global::<Tr>().on_t(|key, _| lag_core::i18n::tr(&key).into());
    ui.set_theme_name(theme.name.clone().into());
    start_donor(&settings);
    let st = Rc::new(RefCell::new(State {
        build: themes::build_dir(&theme),
        // the files the editor changes: a folder theme's own, or the working folder of a .lag file
        dir: theme.dir.clone(),
        theme,
        instance: Rc::default(),
        raw: None,
        sources: Vec::new(),
        selected: None,
        page: Rc::new(std::cell::Cell::new(1)),
        filter: String::new(),
        undo: Vec::new(),
        redo: Vec::new(),
        tokens: Vec::new(),
        mode: Rc::new(RefCell::new(match settings.theme.as_deref() {
            Some(m @ ("light" | "neon")) => m.to_owned(),
            _ => "dark".to_owned(),
        })),
        all_modes: false,
        token_filter: String::new(),
        picker: None,
        imported: Vec::new(),
    }));
    if let Err(e) = load(&ui, &mut st.borrow_mut()) {
        report(&ui, Err(e));
    }
    refresh(&ui, &st.borrow());

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
        set_prop(&ui, &s, &name, &input);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_reset_prop(move |name| {
        let Some(ui) = weak.upgrade() else { return };
        edit_selected(&ui, &s, |_, sel, text| {
            let after = remove_binding(text, &sel.path, sel.offset, &name)?;
            Ok((after, format!("{}: {name} = {}", sel.label, tr("ThemeEditor_Default"))))
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
                let (from, to) = if redo { (edit.before.clone(), edit.after.clone()) } else { (edit.after.clone(), edit.before.clone()) };
                let old = st.selected.clone();
                let _ = apply(&ui, &mut st, &edit.path, from, to, false);
                if redo {
                    st.undo.push(edit)
                } else {
                    st.redo.push(edit)
                }
                if let Some(old) = old {
                    reselect(&mut st, &old);
                }
            }
            // the last change's message no longer says what is in the file
            report(&ui, Ok(String::new()));
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
            // the element around it, found before the change (after a delete its place leads nowhere)
            let parent = crumbs(&sel).into_iter().rev().nth(1);
            let r = std::fs::read_to_string(&sel.path)
                .map_err(|e| e.to_string())
                .and_then(|t| restructure(&t, &sel.path, sel.offset, &op))
                .and_then(|(after, next)| change(&ui, &mut st, &sel.path, after, true).map(|_| next));
            match r {
                Ok(next) => {
                    match (next, parent) {
                        (Some(at), _) => st.selected = st.sources.iter().find(|x| x.path == sel.path && x.offset == at).cloned(),
                        (None, Some(p)) => reselect(&mut st, &p),
                        (None, None) => st.selected = None,
                    }
                    Ok(format!("{}: {}", sel.label, tr(&format!("ThemeEditor_Op_{op}"))))
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
        ui.set_current_page(p);
        // what was picked is on the other page (its frames stayed on the new one: QA 2026-11-22)
        s.borrow_mut().selected = None;
        s.borrow().page.set(p);
        if let Some(i) = live(&s.borrow()) {
            let _ = i.set_global_property("MainVM", "current-page", Value::EnumerationValue("Page".into(), PAGES[p.clamp(0, 3) as usize].into()));
        }
        refresh_soon(&ui, &s);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_rename(move |name| {
        let Some(ui) = weak.upgrade() else { return };
        let theme = s.borrow().theme.clone();
        let result = themes::rename(&theme, &name).map(|t| {
            let msg = lag_core::i18n::format("ThemeEditor_Renamed", &[&t.name]).unwrap_or_default();
            ui.set_theme_name(t.name.clone().into());
            s.borrow_mut().theme.name = t.name;
            msg
        });
        if result.is_err() {
            // the field shows the name the theme still has
            ui.set_theme_name("".into());
            ui.set_theme_name(s.borrow().theme.name.clone().into());
        }
        report(&ui, result);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_set_token(move |index, input| {
        let Some(ui) = weak.upgrade() else { return };
        set_token(&ui, &s, index as usize, &input);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_open_picker_prop(move |name| {
        let Some(ui) = weak.upgrade() else { return };
        use slint::Model;
        let colour = ui.get_props().iter().find(|p| p.name == name).map_or(Color::from_rgb_u8(0, 0, 0), |p| p.swatch);
        s.borrow_mut().picker = Some(Target::Prop(name.to_string()));
        let title = format!("{} · {name}", s.borrow().selected.as_ref().map(|x| x.label.clone()).unwrap_or_default());
        open_picker(&ui, &s.borrow(), colour, &title);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_open_picker_token(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let (colour, title) = {
            let st = s.borrow();
            let Some(t) = st.tokens.get(index as usize) else { return };
            let mode = st.mode.borrow().clone();
            (palette::colour_of(t, &st.tokens, &mode).unwrap_or(Color::from_rgb_u8(0, 0, 0)), format!("{}.{}", t.global, t.name))
        };
        s.borrow_mut().picker = Some(Target::Token(index as usize));
        open_picker(&ui, &s.borrow(), colour, &title);
    });
    ui.on_hex_of(|c| hex(c).into());
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_picker_commit(move |c| {
        let Some(ui) = weak.upgrade() else { return };
        apply_picked(&ui, &s, &hex(c));
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_picker_text(move |t| {
        let Some(ui) = weak.upgrade() else { return };
        apply_picked(&ui, &s, &palette::colour_input(&t));
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_picker_ref(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        // the picker shows the chosen colour from now on (QA 2026-11-22: it stayed on the previous one)
        let shown = {
            let st = s.borrow();
            let mode = st.mode.borrow().clone();
            st.tokens.get(index as usize).and_then(|r| palette::colour_of(r, &st.tokens, &mode))
        };
        if let Some(c) = shown {
            let hsva = c.to_hsva();
            ui.set_picker_hue(hsva.hue.rem_euclid(360.0));
            ui.set_picker_sat(hsva.saturation);
            ui.set_picker_val(hsva.value);
            ui.set_picker_alpha(hsva.alpha);
        }
        let text = {
            let st = s.borrow();
            let Some(r) = st.tokens.get(index as usize) else { return };
            match &st.picker {
                // a colour of the same global names it plainly
                Some(Target::Token(i)) if st.tokens.get(*i).is_some_and(|t| t.global == r.global) => r.name.clone(),
                _ => format!("{}.{}", r.global, r.name),
            }
        };
        apply_picked(&ui, &s, &text);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_choose_image_file(move |name, file| {
        let Some(ui) = weak.upgrade() else { return };
        set_picture(&ui, &s, &name, Path::new(file.as_str()));
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_choose_image(move |name| {
        let (sender, receiver) = std::sync::mpsc::channel();
        let filter = lag_core::i18n::tr("ThemeEditor_ImageFilter").to_owned();
        std::thread::spawn(move || {
            let _ = sender.send(crate::views::settings::os::pick_file(None, &filter, "*.png;*.jpg;*.jpeg;*.svg;*.gif;*.webp", "png"));
        });
        let (s, weak) = (s.clone(), weak.clone());
        let timer = Rc::new(slint::Timer::default());
        let keep = timer.clone();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
            let Ok(result) = receiver.try_recv() else { return };
            keep.stop();
            let (Some(file), Some(ui)) = (result, weak.upgrade()) else { return };
            set_picture(&ui, &s, &name, Path::new(&file));
        });
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_palette_mode_changed(move |m| {
        let Some(ui) = weak.upgrade() else { return };
        *s.borrow().mode.borrow_mut() = m.to_string();
        if let Some(i) = live(&s.borrow()) {
            let _ = i.set_property("theme-mode", Value::String(m));
        }
        refresh_soon(&ui, &s);
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_all_modes_changed(move |v| {
        let Some(ui) = weak.upgrade() else { return };
        s.borrow_mut().all_modes = v;
        refresh(&ui, &s.borrow());
    });
    let (s, weak) = (st.clone(), ui.as_weak());
    ui.on_token_filter_changed(move |f| {
        let Some(ui) = weak.upgrade() else { return };
        s.borrow_mut().token_filter = f.to_string();
        refresh(&ui, &s.borrow());
    });
    ui.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    if ui.show().is_err() {
        return;
    }
    if std::env::var_os("LAG_EDITOR_DEBUG").is_some() {
        let start = std::cell::Cell::new(None::<std::time::Instant>);
        let _ = ui.window().set_rendering_notifier(move |state, _| match state {
            slint::RenderingState::BeforeRendering => start.set(Some(std::time::Instant::now())),
            slint::RenderingState::AfterRendering => {
                if let Some(t) = start.take() {
                    let ms = t.elapsed().as_millis();
                    if ms > 40 {
                        log::info!("[ThemeEditor] frame {ms} ms");
                    }
                }
            }
            _ => {}
        });
    }
    // Lag's data keeps arriving (thumbnails, durations, the settings' hardware probe): follow it
    let follow = slint::Timer::default();
    let s = st.clone();
    follow.start(slint::TimerMode::Repeated, Duration::from_millis(700), move || {
        if let Some(i) = live(&s.borrow()) {
            let t = std::time::Instant::now();
            mirror(&i);
            if std::env::var_os("LAG_EDITOR_DEBUG").is_some() {
                log::info!("[ThemeEditor] mirror {} ms", t.elapsed().as_millis());
            }
        }
    });
    if let (Some(snap), Some(out)) = (headless, shots_dir()) {
        let _ = std::fs::create_dir_all(&out);
        snap.pump_drawing(Duration::from_millis(1500));
        for line in script_lines() {
            match command(&ui, &line) {
                Command::Quit => break,
                Command::Shot(name) => {
                    snap.pump_drawing(Duration::from_millis(200));
                    let file = out.join(format!("{name}.png"));
                    match snap.shot(&file.to_string_lossy()) {
                        Ok(()) => println!("shot {}", file.display()),
                        Err(e) => eprintln!("editor-shots: {e}"),
                    }
                }
                Command::Wait(ms) => snap.pump_drawing(Duration::from_millis(ms)),
            }
        }
        remove_unused_pictures(&st.borrow());
        let _ = themes::pack(&st.borrow().theme);
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            let _ = TerminateProcess(GetCurrentProcess(), 0);
        }
    }
    script(&ui);
    let _ = slint::run_event_loop_until_quit();
    remove_unused_pictures(&st.borrow());
    let _ = themes::pack(&st.borrow().theme);
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}

enum Command {
    Wait(u64),
    Shot(String),
    Quit,
}

fn script_lines() -> Vec<String> {
    let a: Vec<String> = std::env::args().collect();
    let Some(file) = a.iter().position(|x| x == "--editor-script").and_then(|i| a.get(i + 1)) else { return Vec::new() };
    std::fs::read_to_string(file).unwrap_or_default().lines().map(str::to_owned).collect()
}

/// One line of an `--editor-script` (checks without the mouse): `pick X Y`, `set NAME VALUE`, `reset NAME`,
/// `crumb N`, `page N`, `undo`, `redo`, `filter TEXT`, `do OP`, `drag DX DY`, `tab N`, `mode dark|light|neon`,
/// `allmodes 0|1`, `token NAME VALUE`, `click X Y`, `picker-prop NAME`, `picker-token NAME`, `picker-hsv H S V A`,
/// `picker-text TEXT`, `picker-ref Global.name`, `picker-close`, `image NAME FILE`, `wait MS`, `shot NAME` (with
/// `--editor-shots`), `quit`.
fn command(ui: &ThemeEditorWindow, line: &str) -> Command {
    if std::env::var_os("LAG_EDITOR_DEBUG").is_some() {
        log::info!("[ThemeEditor] script: {line}");
    }
    // `#` starts a comment only at the start of a line (`token accent #C0392B` keeps its colour)
    let line = line.trim();
    let line = if line.starts_with('#') { "" } else { line };
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let nums = || -> Vec<f32> { rest.split_whitespace().filter_map(|v| v.parse().ok()).collect() };
    match cmd {
        "pick" => {
            let n = nums();
            ui.invoke_pick(n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0));
        }
        "set" => {
            let (n, v) = rest.split_once(' ').unwrap_or((rest, ""));
            ui.invoke_set_prop(n.into(), v.into());
        }
        "reset" => ui.invoke_reset_prop(rest.into()),
        "do" => ui.invoke_structure(rest.trim().into()),
        "drag" => {
            let n = nums();
            ui.invoke_drag_end(n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0));
        }
        "crumb" => ui.invoke_select_crumb(rest.trim().parse().unwrap_or(0)),
        "page" => ui.invoke_page(rest.trim().parse().unwrap_or(1)),
        "undo" => ui.invoke_undo(),
        "redo" => ui.invoke_redo(),
        "filter" => ui.invoke_filter_changed(rest.into()),
        "tab" => ui.set_tab(rest.trim().parse().unwrap_or(0)),
        "mode" => ui.invoke_palette_mode_changed(rest.trim().into()),
        "allmodes" => ui.invoke_all_modes_changed(rest.trim() == "1"),
        "tokens" => {
            ui.set_token_filter(rest.into());
            ui.invoke_token_filter_changed(rest.into());
        }
        // `token NAME VALUE`: the first colour of that name in the palette
        "token" => {
            let (n, v) = rest.split_once(' ').unwrap_or((rest, ""));
            use slint::Model;
            if let Some(t) = ui.get_tokens().iter().find(|t| t.name == n) {
                ui.invoke_set_token(t.index, v.into());
            }
        }
        // `click X Y`: a mouse click into the editor window itself (nothing reaches the system)
        "click" => {
            use slint::platform::{PointerEventButton, WindowEvent};
            let n = nums();
            let position = slint::LogicalPosition::new(n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0));
            let button = PointerEventButton::Left;
            ui.window().dispatch_event(WindowEvent::PointerMoved { position });
            ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
            ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
        }
        "picker-prop" => ui.invoke_open_picker_prop(rest.trim().into()),
        "picker-token" => {
            use slint::Model;
            if let Some(t) = ui.get_tokens().iter().find(|t| t.name == rest.trim()) {
                ui.invoke_open_picker_token(t.index);
            }
        }
        // `picker-hsv H S V A`: moves the picker there and writes it, as a released drag does
        "picker-hsv" => {
            let n = nums();
            let get = |i: usize, d: f32| n.get(i).copied().unwrap_or(d);
            ui.set_picker_hue(get(0, 0.0));
            ui.set_picker_sat(get(1, 1.0));
            ui.set_picker_val(get(2, 1.0));
            ui.set_picker_alpha(get(3, 1.0));
            ui.invoke_picker_commit(Color::from_hsva(get(0, 0.0), get(1, 1.0), get(2, 1.0), get(3, 1.0)));
        }
        "picker-text" => ui.invoke_picker_text(rest.trim().into()),
        "picker-ref" => {
            use slint::Model;
            if let Some(t) = ui.get_picker_swatches().iter().find(|t| t.name == rest.trim()) {
                ui.invoke_picker_ref(t.index);
            }
        }
        "picker-close" => ui.set_picker_open(false),
        "rename" => ui.invoke_rename(rest.trim().into()),
        // `image NAME FILE`: as if FILE was chosen in the dialog for NAME
        "image" => {
            let (n, f) = rest.split_once(' ').unwrap_or((rest, ""));
            ui.invoke_choose_image_file(n.into(), f.trim().into());
        }
        "wait" => return Command::Wait(rest.trim().parse().unwrap_or(500)),
        "shot" => return Command::Shot(rest.trim().to_owned()),
        "quit" => return Command::Quit,
        "" => return Command::Wait(0),
        other => log::warn!("[ThemeEditor] script: unknown command `{other}`"),
    }
    Command::Wait(700)
}

/// `--editor-script <file>` in the real window: one line after another, on timers.
fn script(ui: &ThemeEditorWindow) {
    let lines = script_lines();
    if lines.is_empty() {
        return;
    }
    fn step(weak: crate::ui_twin::Weak<ThemeEditorWindow>, lines: Rc<Vec<String>>, i: usize) {
        let Some(line) = lines.get(i) else { return };
        let Some(ui) = weak.upgrade() else {
            log::warn!("[ThemeEditor] script: the window is gone");
            return;
        };
        let delay = match command(&ui, line) {
            Command::Quit => {
                let _ = slint::quit_event_loop();
                return;
            }
            Command::Wait(ms) => ms,
            Command::Shot(_) => 0,
        };
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
    fn pictures_come_into_the_theme_once() {
        let root = std::env::temp_dir().join(format!("lag-picture-test-{}", std::process::id()));
        let (theme, build) = (root.join("theme"), root.join("build"));
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        let a = root.join("outside").join("bg.png");
        std::fs::write(&a, [1u8, 2, 3]).unwrap();
        let page = build.join("views").join("library.slint");
        assert_eq!(import_picture(&theme, &build, &a, &page).unwrap(), "../art/bg.png");
        assert!(theme.join("art").join("bg.png").is_file(), "copied into the theme itself");
        assert_eq!(import_picture(&theme, &build, &a, &page).unwrap(), "../art/bg.png", "the same file is not copied again");
        std::fs::write(&a, [9u8]).unwrap();
        assert_eq!(import_picture(&theme, &build, &a, &page).unwrap(), "../art/bg-2.png", "another file of that name");
        assert_eq!(import_picture(&theme, &build, &theme.join("art").join("bg.png"), &build.join("app.slint")).unwrap(), "art/bg.png");
        // one of Lag's own pictures (in the built theme): named, not copied
        let lags = build.join("theme").join("neon.jpg");
        assert_eq!(import_picture(&theme, &build, &lags, &page).unwrap(), "../theme/neon.jpg");
        assert!(!theme.join("art").join("neon.jpg").exists());
        assert!(import_picture(&theme, &build, &root.join("outside").join("x.exe"), &page).is_err());
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(hex(Color::from_rgb_u8(0xC0, 0x39, 0x2B)), "#C0392B");
        assert_eq!(hex(Color::from_argb_u8(0x80, 0, 0xFF, 0x10)), "#00FF1080");
    }

    #[test]
    fn spaces_do_not_make_a_file_the_themes() {
        let lag = "Text { text: \"a  b\"; color: red; }
";
        assert!(same_code(lag, "Text {
    text: \"a  b\";
    color: red;
}
"));
        assert!(!same_code(lag, "Text { text: \"a b\"; color: red; }
"), "spaces inside a string are content");
        assert!(!same_code(lag, "Text { text: \"a  b\"; color: blue; }
"));
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
