//! Runtime half of the UI twin (see build_twin.rs): value conversions, the component registry (one compile of
//! the UI, many windows), and the bits of `slint::ComponentHandle` Lag uses (`global`, `window`, `show`, `as_weak`).
use slint::{ModelExt, ModelRc, SharedString};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::Path;

/// A Rust value that crosses into / out of the interpreted UI.
pub trait TwinValue: Sized {
    fn to_value(self) -> Value;
    fn from_value(v: Value) -> Self;
}

pub fn to_value<T: TwinValue>(v: T) -> Value {
    v.to_value()
}
pub fn from_value<T: TwinValue>(v: Value) -> T {
    T::from_value(v)
}

/// A name the theme does not have (or a wrong type): logged once per name, never a crash.
pub fn missing<T: Default>(name: &str, e: impl std::fmt::Debug) -> T {
    thread_local!(static SEEN: RefCell<std::collections::HashSet<String>> = RefCell::default());
    SEEN.with(|s| {
        if s.borrow_mut().insert(name.to_owned()) {
            log::warn!("[Theme] `{name}`: {e:?}");
        }
    });
    T::default()
}

impl TwinValue for Value {
    fn to_value(self) -> Value {
        self
    }
    fn from_value(v: Value) -> Self {
        v
    }
}
impl TwinValue for () {
    fn to_value(self) -> Value {
        Value::Void
    }
    fn from_value(_: Value) -> Self {}
}
impl TwinValue for bool {
    fn to_value(self) -> Value {
        Value::Bool(self)
    }
    fn from_value(v: Value) -> Self {
        matches!(v, Value::Bool(true))
    }
}
macro_rules! number {
    ($($t:ty),*) => {$(
        impl TwinValue for $t {
            fn to_value(self) -> Value { Value::Number(self as f64) }
            fn from_value(v: Value) -> Self { if let Value::Number(n) = v { n as $t } else { <$t>::default() } }
        }
    )*};
}
number!(f32, i32, i64);
impl TwinValue for SharedString {
    fn to_value(self) -> Value {
        Value::String(self)
    }
    fn from_value(v: Value) -> Self {
        if let Value::String(s) = v {
            s
        } else {
            SharedString::default()
        }
    }
}
impl TwinValue for slint::Image {
    fn to_value(self) -> Value {
        Value::Image(self)
    }
    fn from_value(v: Value) -> Self {
        if let Value::Image(i) = v {
            i
        } else {
            slint::Image::default()
        }
    }
}
impl TwinValue for slint::Brush {
    fn to_value(self) -> Value {
        Value::Brush(self)
    }
    fn from_value(v: Value) -> Self {
        if let Value::Brush(b) = v {
            b
        } else {
            slint::Brush::default()
        }
    }
}
impl TwinValue for slint::Color {
    fn to_value(self) -> Value {
        Value::Brush(slint::Brush::SolidColor(self))
    }
    fn from_value(v: Value) -> Self {
        if let Value::Brush(b) = v {
            b.color()
        } else {
            slint::Color::default()
        }
    }
}
impl<T: TwinValue + Clone + 'static> TwinValue for ModelRc<T> {
    fn to_value(self) -> Value {
        Value::Model(ModelRc::new(self.map(T::to_value)))
    }
    fn from_value(v: Value) -> Self {
        match v {
            Value::Model(m) => ModelRc::new(m.map(T::from_value)),
            _ => ModelRc::default(),
        }
    }
}

/// A global of the UI, reached from any of its windows.
pub trait TwinGlobal<'a> {
    fn get(i: &'a ComponentInstance) -> Self;
}

/// A window of the UI (generated: AppWindow, ToastWindow, ...).
pub trait TwinComponent: Sized + 'static {
    const NAME: &'static str;
    fn wrap(inst: ComponentInstance) -> Self;
    fn instance(&self) -> &ComponentInstance;

    /// Creates the window from the loaded UI (see `load`).
    fn new() -> Result<Self, slint::PlatformError> {
        let def = DEFINITIONS.with(|d| d.borrow().get(Self::NAME).cloned());
        let def = def.ok_or_else(|| slint::PlatformError::Other(format!("UI has no `{}`", Self::NAME)))?;
        Ok(Self::wrap(def.create()?))
    }
    fn global<'a, G: TwinGlobal<'a>>(&'a self) -> G {
        G::get(self.instance())
    }
    fn window(&self) -> &slint::Window {
        slint::ComponentHandle::window(self.instance())
    }
    fn show(&self) -> Result<(), slint::PlatformError> {
        slint::ComponentHandle::show(self.instance())
    }
    fn hide(&self) -> Result<(), slint::PlatformError> {
        slint::ComponentHandle::hide(self.instance())
    }
    fn run(&self) -> Result<(), slint::PlatformError> {
        slint::ComponentHandle::run(self.instance())
    }
    fn as_weak(&self) -> Weak<Self> {
        Weak(slint::ComponentHandle::as_weak(self.instance()), PhantomData)
    }
    fn clone_strong(&self) -> Self {
        Self::wrap(slint::ComponentHandle::clone_strong(self.instance()))
    }
}

/// `slint::Weak` for twin windows.
pub struct Weak<T>(slint::Weak<ComponentInstance>, PhantomData<fn() -> T>);
impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        Weak(self.0.clone(), PhantomData)
    }
}
impl<T> Default for Weak<T> {
    fn default() -> Self {
        Weak(slint::Weak::default(), PhantomData)
    }
}
impl<T: TwinComponent> Weak<T> {
    pub fn upgrade(&self) -> Option<T> {
        self.0.upgrade().map(T::wrap)
    }
    pub fn upgrade_in_event_loop(&self, f: impl FnOnce(T) + Send + 'static) -> Result<(), slint::EventLoopError> {
        self.0.upgrade_in_event_loop(move |i| f(T::wrap(i)))
    }
}

thread_local! {
    static DEFINITIONS: RefCell<HashMap<String, ComponentDefinition>> = RefCell::default();
}

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

/// Compiles a `.slint` entry file; `api` = the folder `@lag/...` imports resolve to. The error text is
/// "file:line: message" lines.
pub fn compile(entry: &Path, api: &Path) -> Result<Vec<ComponentDefinition>, String> {
    let mut c = slint_interpreter::Compiler::default();
    c.set_library_paths([("lag".to_string(), api.to_path_buf())].into_iter().collect());
    let result = spin_on(c.build_from_path(entry));
    let errors: Vec<String> = result
        .diagnostics()
        .filter(|d| d.level() == slint_interpreter::DiagnosticLevel::Error)
        .map(|d| d.to_string())
        .collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(result.components().collect())
}

/// Compiles a UI and makes its windows available to `new()`, replacing everything loaded before.
pub fn load(entry: &Path, api: &Path) -> Result<(), String> {
    let defs = compile(entry, api)?;
    DEFINITIONS.with(|d| d.borrow_mut().clear());
    install(defs);
    Ok(())
}

/// Makes these windows available to `new()`, replacing same-named ones (a theme over Lag's own UI: a window the
/// theme does not draw stays Lag's).
pub fn install(defs: Vec<ComponentDefinition>) {
    DEFINITIONS.with(|d| {
        let mut d = d.borrow_mut();
        for c in defs {
            d.insert(c.name().to_owned(), c);
        }
    });
}

/// The windows loaded now.
pub fn loaded() -> Vec<String> {
    DEFINITIONS.with(|d| d.borrow().keys().cloned().collect())
}
