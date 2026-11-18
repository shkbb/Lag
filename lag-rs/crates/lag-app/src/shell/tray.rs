//! Port of `App.axaml` tray menu and `App.axaml.cs` tray handlers/localization. The menu is our own Slint
//! window (ui/tray_menu.slint) styled like the app, as C#'s Avalonia-rendered tray menu was.
use crate::{app_ctx::AppCtx, AppWindow, MainVM, Page, Tr, TrayMenuWindow};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    io::Cursor,
    rc::{Rc, Weak},
};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

thread_local! { static CONTEXT: RefCell<Weak<AppCtx>> = RefCell::new(Weak::new()); }
pub(super) fn flush_settings() {
    CONTEXT.with(|c| {
        if let Some(ctx) = c.borrow().upgrade() {
            ctx.flush_settings();
        }
    });
}

pub struct Tray {
    _icon: Rc<TrayIcon>,
    _menu: TrayMenuWindow,
}

/// The app icon for the theme, as the hand-drawn entry of `size` px (or the nearest larger one) from the .ico.
/// Owner, 2026-11-17: the icon follows the theme (light = cream + bronze); and the small sizes must be the
/// .ico's own drawings - a 256 px image scaled to 32 made the outer arrow too thin.
pub(crate) fn themed_icon_rgba(dark: bool, size: u32) -> Option<(Vec<u8>, u32, u32)> {
    let bytes: &[u8] = if dark {
        include_bytes!("../../../../../InstantReplay/Assets/icon.ico")
    } else {
        include_bytes!("../../assets/icon-light.ico")
    };
    let dir = ico::IconDir::read(Cursor::new(bytes)).ok()?;
    let entry = dir
        .entries()
        .iter()
        .filter(|e| e.width() >= size)
        .min_by_key(|e| e.width())
        .or_else(|| dir.entries().iter().max_by_key(|e| e.width()))?;
    let image = entry.decode().ok()?;
    Some((image.rgba_data().to_vec(), image.width(), image.height()))
}

fn themed_icon(dark: bool) -> Option<tray_icon::Icon> {
    let (rgba, w, h) = themed_icon_rgba(dark, 32)?;
    tray_icon::Icon::from_rgba(rgba, w, h).ok()
}

thread_local! { static WINDOW_ICONS: std::cell::Cell<[isize; 2]> = const { std::cell::Cell::new([0, 0]) }; }

/// Taskbar / Alt+Tab / title icons of the main window for the theme (WM_SETICON small + big, each from its own
/// .ico entry). Returns false while the window has no HWND yet.
pub(crate) fn apply_window_icon(ui: &AppWindow, dark: bool) -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        DestroyIcon, GetSystemMetrics, SendMessageW, HICON, ICON_BIG, ICON_SMALL, SM_CXICON, SM_CXSMICON, WM_SETICON,
    };
    let Some(hwnd) = super::nofocus::hwnd(ui.window()) else { return false };
    let small = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    let big = unsafe { GetSystemMetrics(SM_CXICON) }.max(32) as u32;
    let make = |size: u32| themed_icon_rgba(dark, size).and_then(|(px, w, h)| hicon_from_rgba(&px, w, h));
    let (Some(s), Some(b)) = (make(small), make(big)) else { return false };
    unsafe {
        let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(s.0 as isize)));
        let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(b.0 as isize)));
    }
    let old = WINDOW_ICONS.with(|c| c.replace([s.0 as isize, b.0 as isize]));
    for h in old {
        if h != 0 {
            unsafe {
                let _ = DestroyIcon(HICON(h as *mut _));
            }
        }
    }
    true
}

fn hicon_from_rgba(rgba: &[u8], w: u32, h: u32) -> Option<windows::Win32::UI::WindowsAndMessaging::HICON> {
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
    if rgba.len() < (w * h * 4) as usize {
        return None;
    }
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(color.0));
            return None;
        }
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (d, s) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
        let mask = CreateBitmap(w as i32, h as i32, 1, 1, None);
        let ii = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii).ok();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}
impl Tray {
    pub fn new(ui: &AppWindow, ctx: &Rc<AppCtx>) -> Result<Self, Box<dyn std::error::Error>> {
        CONTEXT.with(|c| *c.borrow_mut() = Rc::downgrade(ctx));
        let dark = ui.get_is_dark_theme();
        let icon = match themed_icon(dark) {
            Some(i) => i,
            None => {
                let ico =
                    ico::IconDir::read(Cursor::new(include_bytes!("../../../../../InstantReplay/Assets/icon.ico")))?;
                let entry = ico.entries().iter().min_by_key(|e| e.width().abs_diff(32)).ok_or("empty tray icon")?;
                let image = entry.decode()?;
                tray_icon::Icon::from_rgba(image.rgba_data().to_vec(), image.width(), image.height())?
            }
        };
        let icon = Rc::new(TrayIconBuilder::new().with_tooltip("Lag").with_icon(icon).build()?);
        // Tray + window icons follow the RESOLVED theme. Owner, 2026-11-17: with "System" the icon stayed light -
        // the theme was read once at startup, before the window knew the Windows theme. A 1 s watcher keeps both
        // icons on the current theme (also when Windows itself switches light/dark), and retries the window icon
        // until the window has an HWND (start minimized).
        let icon_dark = Rc::new(std::cell::Cell::new(dark));
        let window_dark: Rc<std::cell::Cell<Option<bool>>> = Rc::new(std::cell::Cell::new(None));
        let sync_icons: Rc<dyn Fn()> = {
            let (ui_w, tray_w) = (ui.as_weak(), Rc::downgrade(&icon));
            let (icon_dark, window_dark) = (icon_dark.clone(), window_dark.clone());
            Rc::new(move || {
                let (Some(ui), Some(tray)) = (ui_w.upgrade(), tray_w.upgrade()) else { return };
                let dark = ui.get_is_dark_theme();
                if dark != icon_dark.get() {
                    if let Some(i) = themed_icon(dark) {
                        let _ = tray.set_icon(Some(i));
                        icon_dark.set(dark);
                    }
                }
                if window_dark.get() != Some(dark) && apply_window_icon(&ui, dark) {
                    window_dark.set(Some(dark));
                }
            })
        };
        {
            let watch = Rc::new(slint::Timer::default());
            let keep = watch.clone();
            let tick = sync_icons.clone();
            watch.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
                let _ = &keep;
                tick();
            });
            sync_icons();
        }

        let menu = TrayMenuWindow::new()?;
        menu.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
        menu.set_theme_mode(ctx.settings().theme.as_deref().unwrap_or("dark").into());
        {
            let weak = ui.as_weak();
            let menu_w = menu.as_weak();
            menu.on_activate(move |action| {
                if let Some(m) = menu_w.upgrade() {
                    let _ = m.hide();
                }
                let Some(ui) = weak.upgrade() else { return };
                let vm = ui.global::<MainVM>();
                match action {
                    0 => vm.invoke_save_replay(),
                    1 => {
                        vm.invoke_navigate(Page::Library);
                        vm.invoke_show_main_window();
                    }
                    2 => vm.invoke_show_main_window(),
                    3 => super::exit(),
                    _ => {}
                }
            });
        }
        {
            // close on focus loss, like any menu
            use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
            let menu_w = menu.as_weak();
            menu.window().on_winit_window_event(move |_, event| {
                if matches!(event, winit::event::WindowEvent::Focused(false)) {
                    if let Some(m) = menu_w.upgrade() {
                        let _ = m.hide();
                    }
                }
                EventResult::Propagate
            });
        }

        let weak = ui.as_weak();
        let menu_w = menu.as_weak();
        TrayIconEvent::set_event_handler(Some(move |event| {
            let TrayIconEvent::Click { button, button_state: MouseButtonState::Up, position, .. } = event else {
                return;
            };
            let weak = weak.clone();
            let menu_w = menu_w.clone();
            let _ = slint::invoke_from_event_loop(move || match button {
                MouseButton::Left => {
                    if let Some(ui) = weak.upgrade() {
                        ui.global::<MainVM>().invoke_show_main_window();
                    }
                }
                MouseButton::Right => {
                    if let Some(m) = menu_w.upgrade() {
                        show_menu_at(&m, position.x as i32, position.y as i32);
                    }
                }
                _ => {}
            });
        }));

        let language = RefCell::new(ctx.settings().language);
        let menu_w = menu.as_weak();
        ctx.on_settings_changed(move |s| {
            // the main window resolves the theme ("system" included) once its theme-mode property is updated
            let sync_icons = sync_icons.clone();
            slint::Timer::single_shot(std::time::Duration::from_millis(50), move || sync_icons());
            if let Some(m) = menu_w.upgrade() {
                m.set_theme_mode(s.theme.as_deref().unwrap_or("dark").into());
                if *language.borrow() != s.language {
                    *language.borrow_mut() = s.language.clone();
                    lag_core::i18n::set_language(s.language.as_deref().unwrap_or("en"));
                    let tr = m.global::<Tr>();
                    tr.set_rev(tr.get_rev().wrapping_add(1));
                }
            }
        });
        Ok(Self { _icon: icon, _menu: menu })
    }
}

/// Opens the menu with its bottom-right corner at the cursor (above the taskbar), kept inside the work area,
/// and gives it focus so a click anywhere else closes it.
fn show_menu_at(menu: &TrayMenuWindow, x: i32, y: i32) {
    // shown first so its size is known; repositioned in the same turn, before the first frame is presented
    let _ = menu.show();
    unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;
        let size = menu.window().size();
        let logical = size.to_logical(menu.window().scale_factor());
        let w = (logical.width * scale) as i32;
        let h = (logical.height * scale) as i32;
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let (mut px, mut py) = (x - w, y - h);
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            let r = info.rcWork;
            px = px.clamp(r.left, (r.right - w).max(r.left));
            py = py.clamp(r.top, (r.bottom - h).max(r.top));
        }
        menu.window().set_position(slint::PhysicalPosition::new(px, py));
    }
    if let Some(hwnd) = super::nofocus::hwnd(menu.window()) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
        let _ = self._menu.hide();
    }
}
