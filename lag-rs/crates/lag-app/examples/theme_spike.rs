//! Custom-UI spike (2026-11-22): can Lag's interface come from files loaded at run time?
//! Headless (software renderer, no window). Measures compiling Lag's whole current UI with the interpreter,
//! then loads a completely different theme file, drives it through the same globals (state in, action out),
//! and swaps themes without a restart.
//!   cargo run -p lag-app --example theme_spike --release -- <out dir>
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};
use slint_interpreter::{ComponentInstance, Compiler, Value};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

#[derive(Clone, Copy, Default)]
struct Rgba([u8; 4]);
impl TargetPixel for Rgba {
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        let a = 255 - c.alpha as u16;
        let mix = |d: u8, s: u8| (s as u16 + (d as u16 * a) / 255) as u8;
        self.0 = [mix(self.0[0], c.red), mix(self.0[1], c.green), mix(self.0[2], c.blue), 255];
    }
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 255])
    }
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

fn working_set_mb() -> f64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
    }
    c.WorkingSetSize as f64 / 1_048_576.0
}

fn compiler(ui: &Path) -> Compiler {
    let mut c = Compiler::default();
    c.set_library_paths([("lag".to_string(), ui.to_path_buf())].into_iter().collect());
    c
}

fn build(c: &Compiler, path: &Path) -> Result<slint_interpreter::ComponentDefinition, String> {
    let r = spin_on(c.build_from_path(path));
    let errors: Vec<String> = r.diagnostics().filter(|d| d.level() == slint_interpreter::DiagnosticLevel::Error).map(|d| d.to_string()).collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    r.component("AppWindow").ok_or_else(|| "no AppWindow".into())
}

fn render(window: &MinimalSoftwareWindow, (w, h): (u32, u32), out: &Path) -> Duration {
    let started = Instant::now();
    slint::platform::update_timers_and_animations();
    let mut buf = vec![Rgba::default(); (w * h) as usize];
    window.request_redraw();
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let took = started.elapsed();
    let bytes: Vec<u8> = buf.iter().flat_map(|p| p.0).collect();
    image::save_buffer(out, &bytes, w, h, image::ColorType::Rgba8).expect("png");
    took
}

/// Lag's side of the theme API: what the app would do for ANY theme, through names only.
fn bind(instance: &ComponentInstance, saves: Rc<Cell<u32>>) {
    instance
        .set_global_callback("Tr", "t", |args| match args.first() {
            Some(Value::String(key)) => Value::String(match key.as_str() {
                "Btn_SaveReplay" => "Зберегти повтор".into(),
                k => SharedString::from(k),
            }),
            _ => Value::String(SharedString::default()),
        })
        .expect("Tr.t");
    let set = |name: &str, v: Value| instance.set_global_property("MainVM", name, v).unwrap_or_else(|e| panic!("{name}: {e}"));
    set("is-recording", Value::Bool(true));
    set("buffer-status", Value::String("1:24".into()));
    set("buffer-fill-percent", Value::Number(70.0));
    set("capture-target-name", Value::String("Counter-Strike 2".into()));
    set("status-text", Value::String("Записується Counter-Strike 2".into()));
    instance
        .set_global_callback("MainVM", "save-replay", move |_| {
            saves.set(saves.get() + 1);
            Value::Void
        })
        .expect("save-replay");
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "theme-spike".into()));
    std::fs::create_dir_all(&out).unwrap();
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/themes/spike-rice.slint");

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless { window: window.clone(), start: Instant::now() })).unwrap();
    let mem0 = working_set_mb();

    // 1. Lag's whole current UI (10.5k lines of .slint), compiled at run time instead of build time
    let c = compiler(&ui);
    let t = Instant::now();
    let def = build(&c, &ui.join("app.slint")).expect("app.slint");
    let compile_app = t.elapsed();
    let t = Instant::now();
    let app = def.create().expect("create app");
    let create_app = t.elapsed();
    let saves = Rc::new(Cell::new(0));
    bind(&app, saves.clone());
    let size = (1360, 840);
    window.set_size(PhysicalSize::new(size.0, size.1));
    app.show().unwrap();
    let first_app = render(&window, size, &out.join("1-built-in-ui-interpreted.png"));
    let frames: Vec<Duration> = (0..30).map(|_| render(&window, size, &out.join("1b.png"))).collect();
    let mem_app = working_set_mb();
    app.hide().unwrap();
    drop(app);

    // 2. a completely different theme, from a file, through the same globals
    // Lag wraps the theme: it re-exports the API globals itself, so a theme only imports what it uses
    let wrapper = out.join("theme-wrapper.slint");
    std::fs::write(
        &wrapper,
        format!(
            "export {{ AppWindow }} from \"{}\";\nexport {{ MainVM, Tr }} from \"@lag/globals.slint\";\n",
            theme.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let t = Instant::now();
    let def = build(&compiler(&ui), &wrapper).expect("theme");
    let compile_theme = t.elapsed();
    let theme_ui = def.create().expect("create theme");
    bind(&theme_ui, saves.clone());
    let size = (900, 560);
    window.set_size(PhysicalSize::new(size.0, size.1));
    theme_ui.show().unwrap();
    let first_theme = render(&window, size, &out.join("2-theme-file.png"));
    // the action: a click on the theme's own button reaches Lag
    let at = LogicalPosition::new(660.0, 250.0);
    window.dispatch_event(WindowEvent::PointerMoved { position: at });
    window.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    window.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
    // live state: Lag changes a value, the theme follows
    theme_ui.set_global_property("MainVM", "buffer-fill-percent", Value::Number(25.0)).unwrap();
    theme_ui.set_global_property("MainVM", "is-recording", Value::Bool(false)).unwrap();
    render(&window, size, &out.join("3-theme-state-changed.png"));

    // 3. swap back without a restart (the definition is cached per compiler; here a fresh compile again)
    theme_ui.hide().unwrap();
    drop(theme_ui);
    let t = Instant::now();
    let def = build(&compiler(&ui), &ui.join("app.slint")).expect("app again");
    let again = def.create().expect("create again");
    let swap_back = t.elapsed();
    bind(&again, saves.clone());
    window.set_size(PhysicalSize::new(1360, 840));
    again.show().unwrap();
    render(&window, (1360, 840), &out.join("4-swapped-back.png"));

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let avg = frames.iter().map(|d| ms(*d)).sum::<f64>() / frames.len() as f64;
    println!("compile whole Lag UI (interpreter): {:.0} ms", ms(compile_app));
    println!("create instance: {:.0} ms, first frame: {:.0} ms, next frames avg {avg:.1} ms (software renderer, 1360x840)", ms(create_app), ms(first_app));
    println!("memory: {mem0:.0} MB at start -> {mem_app:.0} MB with the interpreted UI");
    println!("compile + first frame of the theme file: {:.0} + {:.0} ms", ms(compile_theme), ms(first_theme));
    println!("click on the theme's button reached Lag: {} save(s)", saves.get());
    println!("swap back to the built-in UI (compile + create): {:.0} ms", ms(swap_back));
}
