//! Port of `Services/GlobalHotkeyService.cs` and `GlobalHotkeyManager.cs`.
//! Native callbacks only enqueue; all VM callbacks and capture results run on the UI thread.
use crate::{app_ctx::AppCtx, AppWindow, MainVM};
use crossbeam_channel::{bounded, Sender};
use lag_core::settings::SettingsData;
use crate::ComponentHandle;
use std::{
    cell::{OnceCell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS},
        WindowsAndMessaging::*,
    },
};

const REBIND: u32 = WM_APP + 11;
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURE_PENDING: AtomicBool = AtomicBool::new(false);
static CAPTURE_GENERATION: AtomicU64 = AtomicU64::new(0);
static OVERFLOW: AtomicBool = AtomicBool::new(false);
static TRACE: AtomicBool = AtomicBool::new(false);
static UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
type CaptureCallback = Box<dyn FnOnce(CapturedHotkey)>;
thread_local! {
    static CAPTURE: RefCell<Option<CaptureCallback>> = const { RefCell::new(None) };
    static EVENTS: OnceCell<Sender<KeyEvent>> = const { OnceCell::new() };
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedHotkey {
    pub key: String,
    /// SharpHook-compatible names, persisted verbatim in settings.json.
    pub modifiers: String,
    pub cleared: bool,
}
pub fn start_capture(callback: impl FnOnce(CapturedHotkey) + 'static) {
    CAPTURE_GENERATION.fetch_add(1, Ordering::AcqRel);
    CAPTURE_PENDING.store(false, Ordering::Release);
    CAPTURE.with(|c| *c.borrow_mut() = Some(Box::new(callback)));
    CAPTURING.store(true, Ordering::Release);
}
pub fn cancel() {
    CAPTURE_GENERATION.fetch_add(1, Ordering::AcqRel);
    CAPTURE_PENDING.store(false, Ordering::Release);
    CAPTURE.with(|c| {
        c.borrow_mut().take();
    });
    CAPTURING.store(false, Ordering::Release);
    *UNTIL.lock().unwrap() = Some(Instant::now() + Duration::from_millis(800));
}
pub fn are_suppressed() -> bool {
    CAPTURING.load(Ordering::Acquire) || UNTIL.lock().unwrap().is_some_and(|t| Instant::now() < t)
}

/// C# VirtualKeyFromKeyCode's complete registration table (0 = unbound/unmapped).
pub fn virtual_key(key: &str) -> u32 {
    let name = key.strip_prefix("Vc").unwrap_or(key);
    if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() {
        return name.as_bytes()[0].to_ascii_uppercase() as u32;
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return 0x70 + n - 1;
        }
    }
    match name {
        "Space" => 0x20,
        "Enter" => 0x0d,
        "Escape" => 0x1b,
        "Tab" => 9,
        "Backspace" => 8,
        "Delete" => 0x2e,
        "Insert" => 0x2d,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "PrintScreen" => 0x2c,
        _ => 0,
    }
}
fn modifier_mask(text: &str) -> u32 {
    if let Ok(n) = text.parse() {
        return n;
    }
    text.split([',', '|', ' ']).fold(0, |m, name| {
        m | match name {
            "LeftShift" => 1,
            "LeftCtrl" => 2,
            "LeftMeta" => 4,
            "LeftAlt" => 8,
            "RightShift" => 16,
            "RightCtrl" => 32,
            "RightMeta" => 64,
            "RightAlt" => 128,
            "Shift" => 17,
            "Ctrl" => 34,
            "Meta" => 68,
            "Alt" => 136,
            _ => 0,
        }
    })
}
pub fn modifiers_to_win32(text: &str) -> u32 {
    mask_to_win32(modifier_mask(text))
}
fn mask_to_win32(m: u32) -> u32 {
    u32::from(m & 136 != 0)
        | (u32::from(m & 34 != 0) << 1)
        | (u32::from(m & 17 != 0) << 2)
        | (u32::from(m & 68 != 0) << 3)
}
fn modifier_name(mask: u32) -> String {
    let names = [
        (1, "LeftShift"),
        (2, "LeftCtrl"),
        (4, "LeftMeta"),
        (8, "LeftAlt"),
        (16, "RightShift"),
        (32, "RightCtrl"),
        (64, "RightMeta"),
        (128, "RightAlt"),
    ];
    let names: Vec<_> = names.iter().filter(|(bit, _)| mask & bit != 0).map(|(_, n)| *n).collect();
    if names.is_empty() {
        "None".into()
    } else {
        names.join(", ")
    }
}
fn key_name(vk: u32) -> String {
    for name in [
        "Space",
        "Enter",
        "Escape",
        "Tab",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Up",
        "Down",
        "Left",
        "Right",
        "PrintScreen",
    ] {
        if virtual_key(name) == vk {
            return format!("Vc{name}");
        }
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5a => format!("Vc{}", char::from_u32(vk).unwrap()),
        0x70..=0x87 => format!("VcF{}", vk - 0x70 + 1),
        _ => "VcUndefined".into(),
    }
}
fn modifier_bit(vk: u32) -> u32 {
    match vk {
        0xa0 => 1,
        0xa1 => 16,
        0xa2 => 2,
        0xa3 => 32,
        0xa4 => 8,
        0xa5 => 128,
        0x5b => 4,
        0x5c => 64,
        _ => 0,
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Config {
    combos: [(u32, u32); 4],
    required: u32,
    ptt: u32,
    ptt_enabled: bool,
}
impl Config {
    fn from_settings(s: &SettingsData) -> Self {
        let combo = |m: &Option<String>, k: &Option<String>| {
            (modifiers_to_win32(m.as_deref().unwrap_or("None")), virtual_key(k.as_deref().unwrap_or("VcUndefined")))
        };
        Self {
            combos: [
                combo(&s.hotkey_modifiers, &s.hotkey_key),
                combo(&s.screenshot_modifiers, &s.screenshot_key),
                combo(&s.pause_modifiers, &s.pause_key),
                combo(&s.record_modifiers, &s.record_key),
            ],
            required: modifier_mask(s.hotkey_modifiers.as_deref().unwrap_or("None")),
            ptt: virtual_key(s.ptt_key.as_deref().unwrap_or("VcUndefined")),
            ptt_enabled: s.ptt_enabled,
        }
    }
}
#[derive(Clone, Copy)]
struct KeyEvent {
    vk: u32,
    down: bool,
}
unsafe extern "system" fn hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && matches!(w.0 as u32, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
        let data = unsafe { &*(l.0 as *const KBDLLHOOKSTRUCT) };
        if TRACE.load(Ordering::Relaxed) {
            log::info!(
                "[GlobalHotkeyManager] hook vk=0x{:02x} msg=0x{:x} injected={}",
                data.vkCode,
                w.0,
                data.flags.contains(LLKHF_INJECTED)
            );
        }
        let vk = match data.vkCode {
            0x10 => {
                if data.scanCode == 0x36 {
                    0xa1
                } else {
                    0xa0
                }
            }
            0x11 => {
                if data.flags.contains(LLKHF_EXTENDED) {
                    0xa3
                } else {
                    0xa2
                }
            }
            0x12 => {
                if data.flags.contains(LLKHF_EXTENDED) {
                    0xa5
                } else {
                    0xa4
                }
            }
            vk => vk,
        };
        EVENTS.with(|q| {
            if let Some(q) = q.get() {
                if q.try_send(KeyEvent { vk, down: matches!(w.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) }).is_err() {
                    OVERFLOW.store(true, Ordering::Release);
                }
            }
        });
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}
unsafe extern "system" fn probe_proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 {
        let data = unsafe { &*(l.0 as *const KBDLLHOOKSTRUCT) };
        log::info!("[HookProbe] vk=0x{:02x} msg=0x{:x}", data.vkCode, w.0);
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}
/// Diagnostics (gpu-check `hookprobe`): a bare WH_KEYBOARD_LL on its own thread that only logs.
pub fn probe_hook(label: &'static str) {
    std::thread::spawn(move || unsafe {
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        let module = GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).ok();
        match SetWindowsHookExW(WH_KEYBOARD_LL, Some(probe_proc), module, 0) {
            Ok(_) => log::info!("[HookProbe] {label}: installed on thread {}", GetCurrentThreadId()),
            Err(e) => log::warn!("[HookProbe] {label}: failed {e}"),
        }
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
    });
}
fn dispatch(ui: &crate::Weak<AppWindow>, action: usize) {
    let ui = ui.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if are_suppressed() {
            return;
        }
        if let Some(ui) = ui.upgrade() {
            log::info!(
                "[GlobalHotkeyService] {} hotkey pressed.",
                ["save", "screenshot", "pause", "record"].get(action).unwrap_or(&"?")
            );
            let vm = ui.global::<MainVM>();
            match action {
                0 => vm.invoke_save_replay(),
                1 => vm.invoke_take_screenshot(),
                2 => vm.invoke_toggle_pause_recording(),
                3 => vm.invoke_toggle_recording(),
                _ => {}
            }
        }
    });
}
/// winit registers the process for KEYBOARD raw input when its event loop is created (only to emit
/// `DeviceEvent::Key`, which Slint never reads). Windows then stops calling this process's WH_KEYBOARD_LL hook
/// while one of its windows is in the foreground - so recording a new hotkey in Settings (Lag is the active
/// window right then) never saw the key press. Found by the settings audit 2026-11-17 with real injected keys
/// (and a bare PowerShell repro: RegisterRawInputDevices(keyboard) on its own window -> its LL hook gets 0 calls).
/// Dropping the keyboard registration restores the hook; window keyboard input (WM_KEYDOWN/WM_CHAR) is unaffected.
fn release_keyboard_raw_input() {
    use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_REMOVE};
    let dev =
        RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_REMOVE, hwndTarget: Default::default() };
    match unsafe { RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32) } {
        Ok(()) => log::info!(
            "[GlobalHotkeyManager] keyboard raw input released (low-level hook sees keys while Lag is focused)."
        ),
        Err(e) => log::warn!("[GlobalHotkeyManager] could not release keyboard raw input: {e}"),
    }
}
pub struct Hotkeys {
    ids: [Arc<AtomicU32>; 2],
    stop: Sender<()>,
    shutdown: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}
impl Hotkeys {
    pub fn new(ui: &AppWindow, ctx: &Rc<AppCtx>) -> Self {
        release_keyboard_raw_input();
        let config = Arc::new(Mutex::new(Config::from_settings(&ctx.settings())));
        let ids = [Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0))];
        let registered = Arc::new(AtomicU32::new(u32::MAX));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (events, rx) = bounded(1024);
        let (stop, stopped) = bounded(1);
        let mut threads = Vec::new();
        let (c, id, r, weak) = (config.clone(), ids[0].clone(), registered.clone(), ui.as_weak());
        let closing = shutdown.clone();
        threads.push(std::thread::spawn(move || unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            id.store(GetCurrentThreadId(), Ordering::Release);
            let register = || {
                let config = c.lock().unwrap().clone();
                let mut flags = 0;
                for (i, (mods, vk)) in config.combos.into_iter().enumerate() {
                    let _ = UnregisterHotKey(None, 9000 + i as i32);
                    if vk != 0 {
                        match RegisterHotKey(
                            None,
                            9000 + i as i32,
                            HOT_KEY_MODIFIERS(mods | 0x4000),
                            vk,
                        ) {
                            Ok(()) => flags |= 1 << i,
                            Err(e) => log::warn!(
                                "[GlobalHotkeyService] Failed to register {} hotkey. Error: {}",
                                ["save", "screenshot", "pause", "record"][i],
                                e.code().0 as u32 & 0xffff
                            ),
                        }
                    }
                }
                r.store(flags, Ordering::Release);
                if flags != 0 {
                    let [(sm,sk),(hm,hk),(pm,pk),(rm,rk)]=config.combos;
                    log::info!("[GlobalHotkeyService] Registered Win32 hotkeys: save(Mod={sm}, Key={sk}), shot(Mod={hm}, Key={hk}), pause(Mod={pm}, Key={pk}), record(Mod={rm}, Key={rk})");
                }
            };
            register();
            loop {
                if closing.load(Ordering::Acquire) {
                    break;
                }
                let result = GetMessageW(&mut msg, None, 0, 0).0;
                if result <= 0 {
                    if result == -1 {
                        log::warn!("[GlobalHotkeyService] GetMessage error.");
                    }
                    break;
                }
                if msg.message == REBIND {
                    register();
                }
                if msg.message == WM_HOTKEY && (9000..9004).contains(&msg.wParam.0) {
                    dispatch(&weak, msg.wParam.0 - 9000);
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            for i in 0..4 {
                let _ = UnregisterHotKey(None, 9000 + i);
            }
            log::info!("[GlobalHotkeyService] Message loop exited and hotkeys unregistered.");
        }));
        let id = ids[1].clone();
        let closing = shutdown.clone();
        threads.push(std::thread::spawn(move || unsafe {
            let _ = EVENTS.with(|q| q.set(events));
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            id.store(GetCurrentThreadId(), Ordering::Release);
            let module = GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).ok();
            TRACE.store(std::env::var_os("LAG_HOTKEY_TRACE").is_some(), Ordering::Relaxed);
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module, 0) {
                Ok(h) => {
                    if TRACE.load(Ordering::Relaxed) {
                        log::info!("[GlobalHotkeyManager] keyboard hook installed on thread {}.", GetCurrentThreadId());
                    }
                    let mut last = 1;
                    while !closing.load(Ordering::Acquire) {
                        last = GetMessageW(&mut msg, None, 0, 0).0;
                        if last <= 0 {
                            break;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(h);
                    log::info!(
                        "[GlobalHotkeyManager] keyboard hook removed (GetMessage={last}, closing={}).",
                        closing.load(Ordering::Acquire)
                    );
                }
                Err(e) => log::warn!("[GlobalHotkeyManager] hook failed: {e}"),
            }
        }));
        let (c, weak) = (config.clone(), ui.as_weak());
        threads.push(std::thread::spawn(move || {
            let mut mods = 0u32;
            let mut held = [false;256];
            // LAG_HOTKEY_TRACE=1: log every key the hook delivers (hotkey debugging; never on by default)
            let trace = std::env::var_os("LAG_HOTKEY_TRACE").is_some();
            loop {
                if OVERFLOW.swap(false,Ordering::AcqRel) {
                    mods=0;held.fill(false);
                    while rx.try_recv().is_ok() {}
                    let weak=weak.clone();let _=slint::invoke_from_event_loop(move || {if let Some(ui)=weak.upgrade() {ui.global::<MainVM>().invoke_push_to_talk(false);}});
                }
                let event = crossbeam_channel::select! {recv(stopped) -> _ => break, recv(rx) -> e => match e {Ok(e)=>e,Err(_)=>break},default(Duration::from_millis(50))=>continue};
                let bit = modifier_bit(event.vk);
                if trace { log::info!("[GlobalHotkeyManager] key vk=0x{:02x} down={} capturing={}", event.vk, event.down, CAPTURING.load(Ordering::Acquire)); }
                if event.down {mods |= bit;} else {mods &= !bit;}
                let repeated = held.get(event.vk as usize).copied().unwrap_or(false);
                if let Some(h) = held.get_mut(event.vk as usize) {*h = event.down;}
                let cfg = c.lock().unwrap().clone();
                if CAPTURING.load(Ordering::Acquire) && event.down && bit == 0 && !repeated && !CAPTURE_PENDING.swap(true,Ordering::AcqRel) {
                    let generation=CAPTURE_GENERATION.load(Ordering::Acquire);
                    let captured = CapturedHotkey {key:key_name(event.vk),modifiers:modifier_name(mods),cleared:event.vk==0x1b};
                    let _ = slint::invoke_from_event_loop(move || {
                        if CAPTURE_GENERATION.load(Ordering::Acquire)!=generation {return;}
                        let cb = CAPTURE.with(|c| c.borrow_mut().take());
                        if let Some(cb) = cb {cancel(); cb(captured);}
                    });
                    continue;
                }
                if cfg.ptt_enabled && event.vk == cfg.ptt && (!are_suppressed() || !event.down) {
                    if !repeated || !event.down {
                        log::info!("[GlobalHotkeyService] push-to-talk {}.", if event.down { "held" } else { "released" });
                    }
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = weak.upgrade() {if !event.down || !are_suppressed() {ui.global::<MainVM>().invoke_push_to_talk(event.down);}}
                    });
                }
                // Keep SharpHook's required-modifier subset semantics for additional held modifiers.
                // Exact combos have a single authority: RegisterHotKey. Fallback also handles conflicts.
                if event.down && !repeated && cfg.combos[0].1 != 0 && event.vk == cfg.combos[0].1
                    && mods & cfg.required == cfg.required && !are_suppressed()
                    && (mask_to_win32(mods) != cfg.combos[0].0 || registered.load(Ordering::Acquire)&1 == 0) {
                    dispatch(&weak,0);
                }
            }
        }));
        let c = config;
        let id = ids[0].clone();
        let weak = ui.as_weak();
        ctx.on_settings_changed(move |s| {
            let next = Config::from_settings(s);
            let mut old = c.lock().unwrap();
            if *old == next {
                return;
            }
            if old.ptt_enabled && (!next.ptt_enabled || next.ptt != old.ptt) {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<MainVM>().invoke_push_to_talk(false);
                }
            }
            *old = next;
            drop(old);
            let id = id.load(Ordering::Acquire);
            if id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(id, REBIND, WPARAM(0), LPARAM(0));
                }
            }
        });
        Self { ids, stop, shutdown, threads }
    }
}
impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.stop.try_send(());
        for id in &self.ids {
            let id = id.load(Ordering::Acquire);
            if id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
            }
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vk_mapping() {
        for c in b'A'..=b'Z' {
            assert_eq!(virtual_key(&format!("Vc{}", c as char)), c as u32);
        }
        for c in b'0'..=b'9' {
            assert_eq!(virtual_key(&format!("Vc{}", c as char)), c as u32);
        }
        for n in 1..=24 {
            assert_eq!(virtual_key(&format!("VcF{n}")), 0x6f + n);
        }
        for n in ["VcUndefined", "VcF0", "VcF25", "VcLeftShift", ""] {
            assert_eq!(virtual_key(n), 0);
        }
        for vk in [8, 9, 13, 27, 32, 33, 34, 35, 36, 37, 38, 39, 40, 44, 45, 46] {
            assert_eq!(virtual_key(&key_name(vk)), vk);
        }
    }
    #[test]
    fn modifier_mapping() {
        assert_eq!(modifiers_to_win32("LeftAlt"), 1);
        assert_eq!(modifiers_to_win32("RightCtrl, LeftShift"), 6);
        assert_eq!(modifiers_to_win32("LeftMeta | RightAlt | LeftCtrl | RightShift"), 15);
        assert_eq!(modifiers_to_win32("None"), 0);
        assert_eq!(modifiers_to_win32("255"), 15);
        for mask in 0..256 {
            assert_eq!(modifier_mask(&modifier_name(mask)), mask);
        }
    }
    /// Real OS input (F21 via SendInput, a key no app reacts to): run by hand with `-- --ignored`.
    #[test]
    #[ignore]
    fn low_level_hook_sees_injected_keys() {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let (tx, rx) = bounded(64);
        let tid = Arc::new(AtomicU32::new(0));
        let t2 = tid.clone();
        let th = std::thread::spawn(move || unsafe {
            let _ = EVENTS.with(|q| q.set(tx));
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let module = GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).ok();
            let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module, 0).expect("hook");
            t2.store(GetCurrentThreadId(), Ordering::Release);
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(h);
        });
        while tid.load(Ordering::Acquire) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        let ev = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0x84),
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(&[ev(false), ev(true)], std::mem::size_of::<INPUT>() as i32) };
        let got: Vec<_> =
            (0..2).filter_map(|_| rx.recv_timeout(Duration::from_secs(2)).ok()).map(|e| (e.vk, e.down)).collect();
        unsafe {
            let _ = PostThreadMessageW(tid.load(Ordering::Acquire), WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = th.join();
        assert_eq!(got, [(0x84, true), (0x84, false)]);
    }
    #[test]
    fn unbound_stays_unbound() {
        let s = SettingsData { hotkey_key: Some("VcUndefined".into()), ..Default::default() };
        assert_eq!(Config::from_settings(&s).combos[0].1, 0);
    }
}
