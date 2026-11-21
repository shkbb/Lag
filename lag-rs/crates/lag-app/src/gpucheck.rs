//! `Lag.exe --gpu-check <script.txt> <out-dir>`: like `--ui-check`, but in a REAL window with the real GPU
//! (femtovg/OpenGL) renderer, so renderer-only effects can be seen and measured (owner, 2026-11-17: the
//! settings page shifting ~1 px while a dropdown is open never showed in the software renderer).
//! Shows a window for a few seconds. Input is dispatched to the Slint window only (never to the OS).
//! No tray, no hotkeys, no recording engine, no single-instance lock; settings come from %APPDATA%.
//! Commands: size WxH | page settings|library|player|editor | tab N | move X Y | click X Y | down X Y |
//!           up X Y | scroll X Y DY | wait MS | shot NAME (Slint scene only)
//!           screen NAME (what is really on screen over the window: native video + overlay windows too)
//!           editor-clip N | editor-play | editor-crop | editor-rotate | editor-filter N | editor-aspect N
//!           editor-addtext | ov-click X Y | ov-dbl X Y | ov-down X Y | ov-move X Y | ov-up X Y (stage coords,
//!           dispatched to the editor's overlay window) | type TEXT (main window keys) | key esc|enter
//!           rec (start/stop recording) | save (save replay) | choose ID N | toggle ID 0|1 | number ID V | tab N
//!           (settings audit: same paths as the Settings page; the recorder is stopped cleanly at the end)
//!           hotkeys (start the real global hotkey service in this process) | action ID (Settings button, e.g.
//!           save-key starts hotkey capture) | sendkey KEY down|up|tap - a REAL OS key via SendInput, limited to
//!           F13-F24 + left Ctrl/Shift/Alt: keys no app reacts to, so the owner's game/apps are unaffected
use crate::{app_ctx, startup, views, AppWindow, MainVM, Page, SettingsVM, Tr};
use slint::platform::{PointerEventButton, WindowEvent};
use crate::ComponentHandle;
use slint::LogicalPosition;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Duration};

pub fn args() -> Option<(String, String)> {
    let a: Vec<String> = std::env::args().collect();
    let i = a.iter().position(|x| x == "--gpu-check")?;
    Some((a.get(i + 1).cloned().unwrap_or_default(), a.get(i + 2).cloned().unwrap_or_else(|| ".".into())))
}

pub fn run(script: String, dir: String) {
    if log::set_logger(&crate::STDERR_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    crate::views::player::mark_scripted_input();
    let mut settings = startup::load_settings();
    settings.auto_start_recording = false;
    settings.start_minimized = false;
    settings.stats_shown = false;
    lag_core::i18n::set_language(settings.language.as_deref().unwrap_or("en"));
    let Ok(ui) = AppWindow::new() else { return };
    ui.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
    let ctx = app_ctx::AppCtx::new(settings);
    views::init_all(&ui, &ctx);
    CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    let _ = std::fs::create_dir_all(&dir);
    let lines: VecDeque<String> = std::fs::read_to_string(&script)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let queue = Rc::new(RefCell::new(lines));
    if ui.show().is_err() {
        return;
    }
    let weak = ui.as_weak();
    let _keep = ctx.clone();
    slint::Timer::single_shot(Duration::from_millis(1500), move || step(weak, queue, dir));
    let _ = slint::run_event_loop_until_quit();
    // stop a running recording cleanly first: a process killed mid-NVENC once left the driver hung
    crate::views::main_vm::shutdown_recorder();
    HOTKEYS.with(|h| h.borrow_mut().take());
    drop(ui);
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
}

fn step(weak: crate::Weak<AppWindow>, queue: Rc<RefCell<VecDeque<String>>>, dir: String) {
    let Some(ui) = weak.upgrade() else { return };
    let Some(line) = queue.borrow_mut().pop_front() else {
        let _ = slint::quit_event_loop();
        return;
    };
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let a: Vec<f32> = it.clone().filter_map(|v| v.parse().ok()).collect();
    let word = it.next().unwrap_or("").to_string();
    let at = LogicalPosition::new(a.first().copied().unwrap_or(0.0), a.get(1).copied().unwrap_or(0.0));
    let win = ui.window();
    let mut delay = 30u64;
    match cmd {
        "size" => {
            if let Some((w, h)) =
                word.split_once('x').and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)))
            {
                win.set_size(slint::LogicalSize::new(w, h));
                delay = 400;
            }
        }
        "page" => {
            let p = match word.as_str() {
                "library" => Page::Library,
                "player" => Page::Player,
                "editor" => Page::Editor,
                _ => Page::Settings,
            };
            ui.global::<MainVM>().invoke_navigate(p);
            delay = 400;
        }
        "tab" => ui.global::<SettingsVM>().invoke_select_tab(a.first().copied().unwrap_or(0.0) as i32),
        "move" => win.dispatch_event(WindowEvent::PointerMoved { position: at }),
        "down" => win.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left }),
        "up" => win.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left }),
        "click" => {
            win.dispatch_event(WindowEvent::PointerMoved { position: at });
            win.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
            win.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
        }
        "scroll" => win.dispatch_event(WindowEvent::PointerScrolled {
            position: at,
            delta_x: 0.0,
            delta_y: a.get(2).copied().unwrap_or(0.0),
        }),
        "wait" => delay = a.first().copied().unwrap_or(100.0) as u64,
        "editor-clip" => {
            use slint::Model;
            let vm = ui.global::<crate::EditorVM>();
            if let Some(c) = vm.get_clips().row_data(a.first().copied().unwrap_or(0.0) as usize) {
                vm.invoke_open_clip(c.path);
            }
            delay = 300;
        }
        "editor-play" => ui.global::<crate::EditorVM>().invoke_play_pause(),
        "editor-addtext" => ui.global::<crate::EditorVM>().invoke_add_text(),
        "hotkeys" => HOTKEYS.with(|h| {
            if h.borrow().is_none() {
                if let Some(ctx) = CTX.with(|c| c.borrow().clone()) {
                    *h.borrow_mut() = Some(crate::shell::hotkeys::Hotkeys::new(&ui, &ctx));
                    println!("hotkeys started");
                }
            }
        }),
        "hookprobe" => crate::shell::hotkeys::probe_hook("late"),
        "action" => ui.global::<SettingsVM>().invoke_action(word.clone().into()),
        "sendkey" => {
            let mode = line.split_whitespace().nth(2).unwrap_or("tap").to_string();
            match send_key(&word, &mode) {
                Ok(fg) => println!("sendkey {word} {mode} (foreground: {fg})"),
                Err(e) => eprintln!("gpu-check: sendkey {word}: {e}"),
            }
            delay = 120;
        }
        "rec" => {
            ui.global::<MainVM>().invoke_toggle_recording();
            delay = 500;
        }
        "save" => {
            ui.global::<MainVM>().invoke_save_replay();
            delay = 500;
        }
        "choose" | "toggle" | "number" => {
            let mut parts = line.split_whitespace().skip(1);
            let id: slint::SharedString = parts.next().unwrap_or("").into();
            let v = parts.next().unwrap_or("0");
            let vm = ui.global::<SettingsVM>();
            match cmd {
                "choose" => vm.invoke_choose(id, v.parse().unwrap_or(0)),
                "toggle" => vm.invoke_toggle(id, v == "1"),
                _ => vm.invoke_number(id, v.parse().unwrap_or(0.0)),
            }
            delay = 450; // settings save is debounced (350 ms)
        }
        // theme 0 = light, 1 = dark (same path as Settings > General > Theme)
        "theme" => ui.global::<SettingsVM>().invoke_choose("theme".into(), a.first().copied().unwrap_or(1.0) as i32),
        "ov-click" | "ov-dbl" | "ov-down" | "ov-move" | "ov-up" => {
            if let Some(ov) = crate::views::editor::overlay_for_checks() {
                let w = ov.window();
                let b = PointerEventButton::Left;
                let press = |w: &slint::Window| {
                    w.dispatch_event(WindowEvent::PointerPressed { position: at, button: b });
                    w.dispatch_event(WindowEvent::PointerReleased { position: at, button: b });
                };
                w.dispatch_event(WindowEvent::PointerMoved { position: at });
                match cmd {
                    "ov-click" => press(w),
                    "ov-dbl" => {
                        press(w);
                        press(w);
                    }
                    "ov-down" => w.dispatch_event(WindowEvent::PointerPressed { position: at, button: b }),
                    "ov-up" => w.dispatch_event(WindowEvent::PointerReleased { position: at, button: b }),
                    _ => {}
                }
            } else {
                eprintln!("gpu-check: no editor overlay window");
            }
        }
        "type" => {
            for ch in line.split_once(' ').map_or("", |(_, rest)| rest).chars() {
                let t: slint::SharedString = ch.to_string().into();
                win.dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
                win.dispatch_event(WindowEvent::KeyReleased { text: t });
            }
        }
        "key" => {
            let t: slint::SharedString = match word.as_str() {
                "esc" => slint::platform::Key::Escape.into(),
                "enter" => slint::platform::Key::Return.into(),
                other => other.into(),
            };
            win.dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
            win.dispatch_event(WindowEvent::KeyReleased { text: t });
        }
        "editor-crop" => ui.global::<crate::EditorVM>().invoke_toggle_crop_tool(),
        "editor-rotate" => ui.global::<crate::EditorVM>().invoke_rotate_right(),
        "editor-filter" => {
            ui.global::<crate::EditorVM>().invoke_select_filter(a.first().copied().unwrap_or(0.0) as i32)
        }
        "editor-aspect" => {
            ui.global::<crate::EditorVM>().invoke_select_aspect(a.first().copied().unwrap_or(0.0) as i32)
        }
        "screen" => {
            let path = format!("{dir}/{word}.png");
            match crate::shell::nofocus::hwnd(win).and_then(screen_grab) {
                Some((w, h, px)) => {
                    let _ = image::save_buffer(&path, &px, w, h, image::ColorType::Rgba8);
                    println!("screen {path} {w}x{h}");
                }
                None => eprintln!("gpu-check: screen grab failed"),
            }
        }
        "shot" => {
            win.request_redraw();
            match win.take_snapshot() {
                Ok(buf) => {
                    let path = format!("{dir}/{word}.png");
                    let _ =
                        image::save_buffer(&path, buf.as_bytes(), buf.width(), buf.height(), image::ColorType::Rgba8);
                    println!("shot {path} {}x{} scale {}", buf.width(), buf.height(), win.scale_factor());
                }
                Err(e) => eprintln!("gpu-check: snapshot failed: {e}"),
            }
        }
        _ => eprintln!("gpu-check: unknown `{line}`"),
    }
    slint::Timer::single_shot(Duration::from_millis(delay), move || step(weak, queue, dir));
}

thread_local! {
    static CTX: RefCell<Option<Rc<app_ctx::AppCtx>>> = const { RefCell::new(None) };
    static HOTKEYS: RefCell<Option<crate::shell::hotkeys::Hotkeys>> = const { RefCell::new(None) };
}

/// One real key event through SendInput. Only F13-F24 and the left modifiers: no application binds those,
/// so injecting them while the owner plays changes nothing outside Lag. Returns the foreground process name.
fn send_key(key: &str, mode: &str) -> Result<String, String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let vk: u16 = match key.to_ascii_uppercase().as_str() {
        "LCTRL" => 0xA2,
        "LSHIFT" => 0xA0,
        "LALT" => 0xA4,
        k => match k.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
            Some(n @ 13..=24) => 0x70 + n - 1,
            _ => return Err("only F13-F24, LCTRL, LSHIFT, LALT are allowed".into()),
        },
    };
    let ev = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs: Vec<INPUT> = match mode {
        "down" => vec![ev(false)],
        "up" => vec![ev(true)],
        _ => vec![ev(false), ev(true)],
    };
    // from a worker thread, like a real keyboard: the UI thread keeps pumping while the hook chain runs
    let sent = std::thread::spawn(move || unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) })
        .join()
        .unwrap_or(0);
    let inputs_len = if mode == "down" || mode == "up" { 1 } else { 2 };
    if sent as usize != inputs_len {
        return Err(format!("SendInput sent {sent}/{inputs_len}"));
    }
    Ok(foreground_process())
}

fn foreground_process() -> String {
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return format!("pid {pid}") };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let name =
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok()
            {
                String::from_utf16_lossy(&buf[..len as usize]).rsplit(['\\', '/']).next().unwrap_or("").to_string()
            } else {
                format!("pid {pid}")
            };
        let _ = windows::Win32::Foundation::CloseHandle(h);
        name
    }
}

/// What the user sees over `hwnd`'s rect right now (screen DC copy): includes child/owned native windows.
fn screen_grab(hwnd: windows::Win32::Foundation::HWND) -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    unsafe {
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return None;
        }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY).is_ok();
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !ok || lines == 0 {
            return None;
        }
        for c in px.as_chunks_mut::<4>().0 {
            c.swap(0, 2); // BGRA -> RGBA
            c[3] = 255;
        }
        Some((w as u32, h as u32, px))
    }
}
