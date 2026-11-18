//! Dev tool: render any .slint component headlessly (software renderer, no window) to a PNG through the
//! Slint interpreter, so markup can be bisected without recompiling the app.
//! Usage: slint_snap <file.slint> <Component> <out.png> [WxH] [Global.prop=value ...] [move:X,Y] [click:X,Y]
//! Values: true/false, numbers, anything else = string. Events go only to the in-memory window.
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{Platform, WindowAdapter};
use slint_interpreter::ComponentHandle;
use std::rc::Rc;
use std::time::{Duration, Instant};

struct Headless(Rc<MinimalSoftwareWindow>, Instant);
impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.1.elapsed()
    }
}

#[derive(Clone, Copy, Default)]
struct Rgba([u8; 4]);
impl TargetPixel for Rgba {
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        let a = 255 - c.alpha as u16;
        let m = |d: u8, s: u8| (s as u16 + (d as u16 * a) / 255) as u8;
        self.0 = [m(self.0[0], c.red), m(self.0[1], c.green), m(self.0[2], c.blue), 255];
    }
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 255])
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (file, comp, out) = (&a[1], &a[2], &a[3]);
    let (w, h) = a
        .get(4)
        .and_then(|s| s.split_once('x'))
        .map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap()))
        .unwrap_or((1360u32, 840u32));
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(w, h));
    slint::platform::set_platform(Box::new(Headless(window.clone(), Instant::now()))).unwrap();
    let compiler = slint_interpreter::Compiler::default();
    let result = spin_on::spin_on(compiler.build_from_path(file));
    for d in result.diagnostics() {
        eprintln!("{d}");
    }
    let def = result.component(comp).expect("component not found");
    let inst = def.create().expect("create");
    inst.show().unwrap();
    for arg in a.iter().skip(5) {
        if let Some(xy) = arg.strip_prefix("move:").or_else(|| arg.strip_prefix("click:")) {
            let (x, y) = xy
                .split_once(',')
                .map(|(x, y)| (x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0)))
                .unwrap_or((0.0, 0.0));
            let position = slint::LogicalPosition::new(x, y);
            use slint::platform::{PointerEventButton, WindowEvent};
            window.dispatch_event(WindowEvent::PointerMoved { position });
            if arg.starts_with("click:") {
                window.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                window.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
            }
            continue;
        }
        let Some((lhs, v)) = arg.split_once('=') else { continue };
        let Some((global, prop)) = lhs.split_once('.') else { continue };
        let value = match v {
            "true" => slint_interpreter::Value::Bool(true),
            "false" => slint_interpreter::Value::Bool(false),
            _ => v
                .parse::<f64>()
                .map(slint_interpreter::Value::Number)
                .unwrap_or_else(|_| slint_interpreter::Value::String(v.into())),
        };
        if let Err(e) = inst.set_global_property(global, prop, value) {
            eprintln!("set {lhs}: {e}");
        }
    }
    let t = Instant::now();
    let wait = std::env::var("SNAP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(300u64);
    while t.elapsed() < Duration::from_millis(wait) {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(Duration::from_millis(16));
    }
    let mut buf = vec![Rgba::default(); (w * h) as usize];
    window.request_redraw();
    window.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let bytes: Vec<u8> = buf.iter().flat_map(|p| p.0).collect();
    image::save_buffer(out, &bytes, w, h, image::ColorType::Rgba8).unwrap();
    println!("wrote {out}");
}
