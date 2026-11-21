//! Build-time generator of the UI "twin" (docs/spec/themes.md, phase 1).
//!
//! Lag's Rust code talks to its UI through the typed API that `slint-build` generates (`ui.global::<MainVM>()
//! .set_is_recording(..)`, `on_save_replay(..)`, structs, enums). For themes the UI must come from files at run
//! time (`slint-interpreter`), whose API is by name (`set_global_property("MainVM", "is-recording", Value)`). This
//! generator reads the same `.slint` with the interpreter's compiler and writes a Rust module with the SAME typed
//! API on top of the interpreter, so the ~270 call sites keep their shape. Output: `$OUT_DIR/ui_twin.rs`, used
//! with the hand-written runtime in `src/ui_twin/rt.rs`.
use i_slint_compiler::langtype::{StructName, Type};
use i_slint_compiler::object_tree::PropertyVisibility;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

fn spin_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

const KEYWORDS: &[&str] = &["type", "match", "move", "ref", "self", "loop", "in", "fn", "mod", "use", "where", "impl"];

fn snake(name: &str) -> String {
    let s = name.replace('-', "_");
    if KEYWORDS.contains(&s.as_str()) {
        format!("r#{s}")
    } else {
        s
    }
}

fn camel(name: &str) -> String {
    name.split(['-', '_'])
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

#[derive(Default)]
struct Gen {
    structs: BTreeMap<String, std::sync::Arc<i_slint_compiler::langtype::Struct>>,
    enums: BTreeMap<String, std::sync::Arc<i_slint_compiler::langtype::Enumeration>>,
}

impl Gen {
    /// The Rust type for a .slint type; unknown kinds stay a raw `Value`.
    fn rust(&mut self, ty: &Type) -> String {
        match ty {
            Type::Float32 | Type::PhysicalLength | Type::LogicalLength | Type::Rem | Type::Angle | Type::Percent => "f32".into(),
            Type::Int32 => "i32".into(),
            Type::Duration => "i64".into(),
            Type::String => "slint::SharedString".into(),
            Type::Bool => "bool".into(),
            Type::Color => "slint::Color".into(),
            Type::Brush => "slint::Brush".into(),
            Type::Image => "slint::Image".into(),
            Type::Void => "()".into(),
            Type::Array(inner) => format!("slint::ModelRc<{}>", self.rust(inner)),
            Type::Struct(s) => match &s.name {
                StructName::User { name, .. } => {
                    let n = camel(name);
                    if !self.structs.contains_key(&n) {
                        self.structs.insert(n.clone(), s.clone());
                        for f in s.fields.values() {
                            self.rust(f);
                        }
                    }
                    n
                }
                _ => "slint_interpreter::Value".into(),
            },
            Type::Enumeration(e) if e.node.is_some() => {
                let n = camel(&e.name);
                self.enums.entry(n.clone()).or_insert_with(|| e.clone());
                n
            }
            _ => "slint_interpreter::Value".into(),
        }
    }

    /// Accessors for one property / callback. `target` = how to reach it on `self.0` (global or component).
    fn member(&mut self, out: &mut String, owner: Option<&str>, name: &str, ty: &Type, vis: PropertyVisibility) {
        let (get, set, invoke, cb) = match owner {
            Some(g) => (
                format!("self.0.get_global_property({g:?}, {name:?})"),
                format!("self.0.set_global_property({g:?}, {name:?}, v)"),
                format!("self.0.invoke_global({g:?}, {name:?}, &args)"),
                format!("self.0.set_global_callback({g:?}, {name:?}, h)"),
            ),
            None => (
                format!("self.inst.get_property({name:?})"),
                format!("self.inst.set_property({name:?}, v)"),
                format!("self.inst.invoke({name:?}, &args)"),
                format!("self.inst.set_callback({name:?}, h)"),
            ),
        };
        let id = snake(name);
        match ty {
            Type::Callback(f) | Type::Function(f) => {
                let args: Vec<String> = f.args.iter().map(|a| self.rust(a)).collect();
                let ret = self.rust(&f.return_type);
                let params: String = args.iter().enumerate().map(|(i, t)| format!(", a{i}: {t}")).collect();
                let to_vals: String = (0..args.len()).map(|i| format!("rt::to_value(a{i}),")).collect();
                let _ = writeln!(
                    out,
                    "    pub fn invoke_{id}(&self{params}) -> {ret} {{ let args = [{to_vals}]; rt::from_value({invoke}.unwrap_or_else(|e| rt::missing({name:?}, e))) }}"
                );
                if matches!(ty, Type::Callback(_)) {
                    let froms: String = args.iter().enumerate().map(|(i, _)| format!("rt::from_value(a[{i}].clone()),")).collect();
                    let sig: String = args.join(", ");
                    let a = if args.is_empty() { "_a" } else { "a" };
                    let _ = writeln!(
                        out,
                        "    pub fn on_{id}(&self, f: impl FnMut({sig}) -> {ret} + 'static) {{ let f = std::cell::RefCell::new(f); let h = move |{a}: &[slint_interpreter::Value]| rt::to_value((f.borrow_mut())({froms})); if let Err(e) = {cb} {{ rt::missing({name:?}, e) }} }}"
                    );
                }
            }
            _ => {
                let t = self.rust(ty);
                let _ = writeln!(
                    out,
                    "    pub fn get_{id}(&self) -> {t} {{ rt::from_value({get}.unwrap_or_else(|e| rt::missing({name:?}, e))) }}"
                );
                if !matches!(vis, PropertyVisibility::Output) {
                    let _ = writeln!(
                        out,
                        "    pub fn set_{id}(&self, v: {t}) {{ let v = rt::to_value(v); if let Err(e) = {set} {{ rt::missing({name:?}, e) }} }}"
                    );
                }
            }
        }
    }
}

/// The theme API reference (docs/themes/API.md, shipped to theme authors): read straight from `ui/api/globals.slint`
/// and `ui/api/types.slint`, with the comments written above / beside each line as the description.
pub fn api_reference(ui: &Path) -> String {
    let mut out = String::from(
        "# Lag theme API v1\n\n\
         _Generated from `ui/api/` - do not edit._ A theme imports these from `\"@lag/api.slint\"`:\n\n\
         ```slint\nimport { MainVM, LibraryVM, Tr } from \"@lag/api.slint\";\n```\n\n\
         - **in property**: Lag sets it, the theme shows it.\n\
         - **in-out property**: either side may change it (e.g. the current page).\n\
         - **callback**: the theme calls it, Lag does the work (save a replay, open a clip...). A `pure callback` \
         is answered by Lag (`Tr.t(key, Tr.rev)` = the translated text).\n\n\
         Lag's own building blocks can be used too: `@lag/components/...` (buttons, cards, settings rows, the clip \
         card...), `@lag/theme/tokens.slint` (colours, sizes) and `@lag/theme/icons.slint`. The quickest start: \
         Settings > General > Interface > **New theme** (a full copy of Lag's interface) and **Preview**.\n",
    );
    for (file, title) in [("globals.slint", "Globals"), ("types.slint", "Data types")] {
        let text = std::fs::read_to_string(ui.join("api").join(file)).unwrap_or_default();
        let _ = write!(out, "\n## {title}\n");
        let mut comment: Vec<String> = Vec::new();
        let mut in_block = false;
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(c) = line.strip_prefix("//") {
                comment.push(c.trim().to_owned());
                continue;
            }
            if line.is_empty() {
                if !in_block {
                    comment.clear();
                }
                continue;
            }
            let (code, trailing) = match line.find("//") {
                Some(i) => (line[..i].trim(), line[i + 2..].trim()),
                None => (line, ""),
            };
            let mut note = comment.join(" ");
            if !trailing.is_empty() {
                if !note.is_empty() {
                    note.push(' ');
                }
                note.push_str(trailing);
            }
            comment.clear();
            let note = note.replace('|', "\\|");
            if let Some(rest) = code.strip_prefix("export global ").or_else(|| code.strip_prefix("export struct ")).or_else(|| code.strip_prefix("export enum ")) {
                let name = rest.split(|c: char| c == '{' || c.is_whitespace()).next().unwrap_or("");
                let _ = write!(out, "\n### {name}\n\n");
                if !note.is_empty() {
                    let _ = writeln!(out, "{note}\n");
                }
                // a one-line struct / enum: `export struct X { a: t, b: t }`
                if let (Some(a), Some(b)) = (rest.find('{'), rest.rfind('}')) {
                    let body = &rest[a + 1..b];
                    out.push_str("| Name | Type |\n|---|---|\n");
                    for f in body.split(',').map(str::trim).filter(|f| !f.is_empty()) {
                        let (n, t) = f.split_once(':').unwrap_or((f, ""));
                        let _ = writeln!(out, "| `{}` | `{}` |", n.trim(), t.trim());
                    }
                    continue;
                }
                in_block = true;
                out.push_str("| Name | Kind | Type | Notes |\n|---|---|---|---|\n");
                continue;
            }
            if code.starts_with('}') {
                in_block = false;
                continue;
            }
            if !in_block {
                continue;
            }
            let code = code.trim_end_matches([';', ',']);
            let pure = code.starts_with("pure ");
            let code = code.trim_start_matches("pure ");
            let row = if let Some(rest) = ["in-out property", "in property", "out property"].iter().find_map(|k| code.strip_prefix(k).map(|r| (k, r))) {
                let (kind, rest) = rest;
                let (ty, rest) = rest.trim().strip_prefix('<').and_then(|r| r.split_once('>')).unwrap_or(("", rest));
                let name = rest.split(':').next().unwrap_or("").trim();
                (name.to_owned(), kind.to_string(), ty.to_owned())
            } else if let Some(rest) = code.strip_prefix("callback ") {
                let (name, sig) = rest.split_once('(').unwrap_or((rest, ")"));
                let sig = format!("({sig}");
                // keep the argument names written as /* comments */
                (name.trim().to_owned(), if pure { "pure callback".into() } else { "callback".into() }, sig.replace("/*", "").replace("*/", ""))
            } else if code.contains(':') && !code.contains('(') {
                // a struct field / an enum value list
                let (n, t) = code.split_once(':').unwrap_or((code, ""));
                (n.trim().to_owned(), "field".into(), t.trim().to_owned())
            } else {
                continue;
            };
            let _ = writeln!(out, "| `{}` | {} | `{}` | {} |", row.0, row.1, row.2.replace('|', "\\|"), note);
        }
    }
    out
}

pub fn generate(root: &Path, out_file: &Path) {
    let compiler = slint_interpreter::Compiler::default();
    let result = spin_on(compiler.build_from_path(root));
    let errors: Vec<String> = result
        .diagnostics()
        .filter(|d| d.level() == slint_interpreter::DiagnosticLevel::Error)
        .map(|d| d.to_string())
        .collect();
    if !errors.is_empty() {
        panic!("ui twin: {}", errors.join("\n"));
    }
    let mut g = Gen::default();
    let mut out = String::from("// @generated by build_twin.rs from ui/app.slint - do not edit\n");
    let components: Vec<_> = result.components().collect();

    // globals (seen through the first component: every exported component shares them)
    if let Some(first) = components.first() {
        for global in first.globals() {
            let ty = camel(&global);
            let _ = writeln!(out, "pub struct {ty}<'a>(&'a slint_interpreter::ComponentInstance);");
            let _ = writeln!(out, "impl<'a> rt::TwinGlobal<'a> for {ty}<'a> {{ fn get(i: &'a slint_interpreter::ComponentInstance) -> Self {{ {ty}(i) }} }}");
            let _ = writeln!(out, "impl {ty}<'_> {{");
            let mut members: Vec<_> = first.global_properties_and_callbacks(&global).into_iter().flatten().collect();
            members.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, (t, vis)) in members {
                g.member(&mut out, Some(&global), &name, &t, vis);
            }
            out.push_str("}\n");
        }
    }

    // components (windows)
    for c in &components {
        let ty = camel(c.name());
        let _ = writeln!(out, "pub struct {ty} {{ inst: slint_interpreter::ComponentInstance }}");
        let _ = writeln!(
            out,
            "impl rt::TwinComponent for {ty} {{ const NAME: &'static str = {:?}; fn wrap(inst: slint_interpreter::ComponentInstance) -> Self {{ Self {{ inst }} }} fn instance(&self) -> &slint_interpreter::ComponentInstance {{ &self.inst }} }}",
            c.name()
        );
        let _ = writeln!(out, "impl {ty} {{");
        let mut members: Vec<_> = c.properties_and_callbacks().collect();
        members.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, (t, vis)) in members {
            g.member(&mut out, None, &name, &t, vis);
        }
        out.push_str("}\n");
    }

    // structs and enums met on the way (a struct's fields can add more)
    let mut done = std::collections::BTreeSet::new();
    loop {
        let pending: Vec<_> = g.structs.iter().filter(|(n, _)| !done.contains(*n)).map(|(n, s)| (n.clone(), s.clone())).collect();
        if pending.is_empty() {
            break;
        }
        for (name, s) in pending {
            done.insert(name.clone());
            let fields: Vec<(String, String, String)> =
                s.fields.iter().map(|(f, t)| (f.to_string(), snake(f), g.rust(t))).collect();
            let _ = writeln!(out, "#[derive(Clone, Debug, Default, PartialEq)]\npub struct {name} {{");
            for (_, id, t) in &fields {
                let _ = writeln!(out, "    pub {id}: {t},");
            }
            out.push_str("}\n");
            let _ = writeln!(out, "impl rt::TwinValue for {name} {{");
            out.push_str("    fn to_value(self) -> slint_interpreter::Value { let mut s = slint_interpreter::Struct::default();\n");
            for (f, id, _) in &fields {
                let _ = writeln!(out, "        s.set_field({f:?}.into(), rt::to_value(self.{id}));");
            }
            out.push_str("        slint_interpreter::Value::Struct(s) }\n");
            out.push_str("    fn from_value(v: slint_interpreter::Value) -> Self { let slint_interpreter::Value::Struct(s) = v else { return Self::default() }; Self {\n");
            for (f, id, _) in &fields {
                let _ = writeln!(out, "        {id}: s.get_field({f:?}).cloned().map(rt::from_value).unwrap_or_default(),");
            }
            out.push_str("    } }\n}\n");
        }
    }
    for (name, e) in &g.enums {
        let _ = writeln!(out, "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]\npub enum {name} {{");
        for (i, v) in e.values.iter().enumerate() {
            if i == e.default_value {
                out.push_str("    #[default]\n");
            }
            let _ = writeln!(out, "    {},", camel(v));
        }
        out.push_str("}\n");
        let _ = writeln!(out, "impl rt::TwinValue for {name} {{");
        let _ = writeln!(out, "    fn to_value(self) -> slint_interpreter::Value {{ slint_interpreter::Value::EnumerationValue({:?}.into(), match self {{", e.name.as_str());
        for v in &e.values {
            let _ = writeln!(out, "        Self::{} => {:?},", camel(v), v.as_str());
        }
        out.push_str("    }.into()) }\n");
        out.push_str("    fn from_value(v: slint_interpreter::Value) -> Self { match v { slint_interpreter::Value::EnumerationValue(_, v) => match v.as_str() {\n");
        for v in &e.values {
            let _ = writeln!(out, "        {:?} => Self::{},", v.as_str(), camel(v));
        }
        out.push_str("        _ => Self::default() }, _ => Self::default() } }\n}\n");
    }
    std::fs::write(out_file, out).expect("write ui_twin.rs");
}
