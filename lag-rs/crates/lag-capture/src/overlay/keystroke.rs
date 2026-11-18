//! Port of `Services/VfrCapture/KeystrokeTracker.cs`; native hooks replace SharpHook's feed.
use std::cell::{OnceCell, UnsafeCell};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::core::Result;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const FADE_MS: f64 = 450.0;
const CAPACITY: usize = 1024;
#[derive(Clone, Copy, Default)]
struct Event {
    code: u32,
    down: bool,
    at: f64,
}
struct Queue {
    slots: [UnsafeCell<Event>; CAPACITY],
    read: AtomicUsize,
    write: AtomicUsize,
    overflow: AtomicBool,
    stopping: AtomicBool,
}
// SAFETY: one hook/message thread writes, one compositor thread reads; release/acquire publishes
// slots, and the producer never touches a slot until the consumer releases it.
unsafe impl Sync for Queue {}
impl Queue {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| UnsafeCell::new(Event::default())),
            read: AtomicUsize::new(0),
            write: AtomicUsize::new(0),
            overflow: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        }
    }
    fn push(&self, code: u32, down: bool) {
        let write = self.write.load(Ordering::Relaxed);
        if write.wrapping_sub(self.read.load(Ordering::Acquire)) >= CAPACITY {
            self.overflow.store(true, Ordering::Release);
            return;
        }
        unsafe {
            *self.slots[write % CAPACITY].get() = Event { code, down, at: now_ms() };
        }
        self.write.store(write.wrapping_add(1), Ordering::Release);
    }
    fn pop(&self) -> Option<Event> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return None;
        }
        let event = unsafe { *self.slots[read % CAPACITY].get() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(event)
    }
}
thread_local! { static EVENTS: OnceCell<Arc<Queue>> = const { OnceCell::new() }; }
unsafe extern "system" fn keyboard(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && matches!(w.0 as u32, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
        let event = unsafe { &*(l.0 as *const KBDLLHOOKSTRUCT) };
        // Software-injected keys (G HUB/macros/remappers) are not the player's presses: they showed up as
        // "random" lit keys in recorded clips (owner, 2026-11-16).
        if event.flags.contains(LLKHF_INJECTED) {
            return unsafe { CallNextHookEx(None, code, w, l) };
        }
        EVENTS.with(|q| {
            if let Some(q) = q.get() {
                q.push(
                    event.vkCode | if event.flags.contains(LLKHF_EXTENDED) { 0x100 } else { 0 },
                    matches!(w.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN),
                );
            }
        });
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}
unsafe extern "system" fn mouse(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 {
        let button = match w.0 as u32 {
            WM_LBUTTONDOWN | WM_LBUTTONUP => 1,
            WM_RBUTTONDOWN | WM_RBUTTONUP => 2,
            WM_MBUTTONDOWN | WM_MBUTTONUP => 3,
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let e = unsafe { &*(l.0 as *const MSLLHOOKSTRUCT) };
                3 + (e.mouseData >> 16)
            }
            _ => 0,
        };
        let injected = unsafe { &*(l.0 as *const MSLLHOOKSTRUCT) }.flags & LLMHF_INJECTED != 0;
        if button != 0 && !injected {
            EVENTS.with(|q| {
                if let Some(q) = q.get() {
                    q.push(
                        0x10000 + button,
                        matches!(w.0 as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN),
                    );
                }
            });
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}
fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|v| v.as_secs_f64() * 1000.0).unwrap_or(0.0)
}
#[derive(Clone)]
struct Entry {
    label: String,
    up: Option<f64>,
    /// Win32 virtual key used to verify a held key is still physically down (0 = unknown).
    vk: u32,
    down_at: f64,
}
#[derive(Default)]
struct State {
    entries: Vec<Entry>,
}
impl State {
    #[cfg(test)]
    fn push(&mut self, label: String, down: bool, at: f64) {
        self.push_vk(label, down, at, 0);
    }
    fn push_vk(&mut self, label: String, down: bool, at: f64, vk: u32) {
        let found = self.entries.iter_mut().find(|e| e.label == label);
        if down {
            if let Some(e) = found {
                if e.up.is_some() {
                    e.down_at = at;
                }
                e.up = None;
                if vk != 0 {
                    e.vk = vk;
                }
            } else {
                if self.entries.len() >= 10 {
                    self.entries.remove(0);
                }
                self.entries.push(Entry { label, up: None, vk, down_at: at });
            }
        } else if let Some(e) = found {
            e.up = Some(at);
        }
    }
    fn snapshot(&mut self, now: f64) -> Vec<(String, f32)> {
        self.entries.retain(|e| e.up.is_none_or(|up| now - up <= FADE_MS));
        self.entries
            .iter()
            .map(|e| {
                (e.label.clone(), e.up.map(|up| (1.0 - (now - up) / FADE_MS).clamp(0.0, 1.0) as f32).unwrap_or(1.0))
            })
            .collect()
    }
}
/// A hook owner with no input filtering and no rendering locks on its message thread.
/// Dropping joins the thread after it has uninstalled both hooks.
pub struct KeystrokeTracker {
    queue: Arc<Queue>,
    state: State,
    last_reconcile: f64,
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}
impl KeystrokeTracker {
    pub fn new() -> Result<Self> {
        let queue = Arc::new(Queue::new());
        let producer = queue.clone();
        let (ready, receive) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("overlay input hooks".into())
            .spawn(move || {
                // Low-level hooks that answer late get silently removed by Windows (LowLevelHooksTimeout):
                // never let a game starve this thread.
                unsafe {
                    let _ = windows::Win32::System::Threading::SetThreadPriority(
                        windows::Win32::System::Threading::GetCurrentThread(),
                        windows::Win32::System::Threading::THREAD_PRIORITY_TIME_CRITICAL,
                    );
                }
                EVENTS.with(|q| {
                    let _ = q.set(producer.clone());
                });
                let result = unsafe {
                    // Force creation of the message queue before publishing the thread ID.
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                    (|| -> Result<_> {
                        let module = HINSTANCE(GetModuleHandleW(None)?.0);
                        let key = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), Some(module), 0)?;
                        match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), Some(module), 0) {
                            Ok(mouse) => Ok((key, mouse)),
                            Err(e) => {
                                let _ = UnhookWindowsHookEx(key);
                                Err(e)
                            }
                        }
                    })()
                };
                match result {
                    Ok((key, mouse)) => {
                        let sent = ready.send(Ok(unsafe { GetCurrentThreadId() })).is_ok();
                        if sent {
                            unsafe {
                                let mut msg = MSG::default();
                                while !producer.stopping.load(Ordering::Acquire)
                                    && GetMessageW(&mut msg, None, 0, 0).0 > 0
                                {
                                    let _ = TranslateMessage(&msg);
                                    DispatchMessageW(&msg);
                                }
                            }
                        }
                        unsafe {
                            let _ = UnhookWindowsHookEx(mouse);
                            let _ = UnhookWindowsHookEx(key);
                        }
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                    }
                }
            })
            .map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))?;
        match receive.recv() {
            Ok(Ok(thread_id)) => {
                Ok(Self { queue, state: State::default(), last_reconcile: 0.0, thread: Some(thread), thread_id })
            }
            other => {
                let _ = thread.join();
                Err(other.ok().and_then(|r| r.err()).unwrap_or_else(windows::core::Error::from_thread))
            }
        }
    }
    pub fn snapshot(&mut self) -> Vec<(String, f32)> {
        while let Some(e) = self.queue.pop() {
            let (label, vk) = if e.code >= 0x10000 {
                let b = e.code - 0x10000;
                (Some(mouse_label(b)), mouse_vk(b))
            } else {
                (virtual_key_label(e.code), e.code & 0xff)
            };
            if let Some(label) = label {
                self.state.push_vk(label, e.down, e.at, vk);
            }
        }
        if self.queue.overflow.swap(false, Ordering::AcqRel) {
            self.state.entries.clear();
        }
        let now = now_ms();
        // A lost key-up (hook starved while a game loads the CPU, focus changes) used to leave a key lit until
        // pressed again. Held keys are re-checked against the real keyboard state, at most twice a second.
        if now - self.last_reconcile >= 500.0 {
            self.last_reconcile = now;
            for e in self.state.entries.iter_mut() {
                if e.up.is_none() && e.vk != 0 && now - e.down_at > 300.0 {
                    let down = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(e.vk as i32) }
                        as u16
                        & 0x8000
                        != 0;
                    if !down {
                        e.up = Some(now);
                    }
                }
            }
        }
        self.state.snapshot(now)
    }
}
impl Drop for KeystrokeTracker {
    fn drop(&mut self) {
        self.queue.stopping.store(true, Ordering::Release);
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn mouse_vk(button: u32) -> u32 {
    match button {
        1 => 0x01,
        2 => 0x02,
        3 => 0x04,
        4 => 0x05,
        5 => 0x06,
        _ => 0,
    }
}
fn mouse_label(button: u32) -> String {
    match button {
        1 => "LMB".into(),
        2 => "RMB".into(),
        3 => "MMB".into(),
        _ => format!("M{button}"),
    }
}
/// SharpHook label conversion, including its permissive fallback guards.
pub fn key_label(name: &str) -> Option<String> {
    let name = name.strip_prefix("Vc")?;
    let label = match name {
        "LeftShift" | "RightShift" => "Shift",
        "LeftControl" | "RightControl" => "Ctrl",
        "LeftAlt" | "RightAlt" => "Alt",
        "LeftMeta" | "RightMeta" => "Win",
        "Space" => "Space",
        "Enter" => "Enter",
        "Backspace" => "Bksp",
        "Escape" => "Esc",
        "Tab" => "Tab",
        "CapsLock" => "Caps",
        "Up" => "↑",
        "Down" => "↓",
        "Left" => "←",
        "Right" => "→",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "Minus" => "-",
        "Equals" => "=",
        "OpenBracket" => "[",
        "CloseBracket" => "]",
        "Backquote" | "BackQuote" => "`",
        _ if name.len() == 1
            || (name.len() == 2 && (name.starts_with('1') || name.as_bytes()[1].is_ascii_digit()))
            || (name.starts_with('F') && name.len() <= 3) =>
        {
            name
        }
        _ => return None,
    };
    Some(label.into())
}
// SharpHook 5.3.9 exposes virtual-key labels. Extended Enter is NumPadEnter,
// which C# deliberately does not badge. Decode only on the compositor thread.
fn virtual_key_label(code: u32) -> Option<String> {
    let vk = code & 0xff;
    if vk == 0x0d && code & 0x100 != 0 {
        return None;
    }
    if (0x30..=0x39).contains(&vk) || (0x41..=0x5a).contains(&vk) {
        return char::from_u32(vk).map(|c| c.to_string());
    }
    if (0x70..=0x87).contains(&vk) {
        return Some(format!("F{}", vk - 0x6f));
    }
    let name = match vk {
        0x10 | 0xa0 | 0xa1 => "LeftShift",
        0x11 | 0xa2 | 0xa3 => "LeftControl",
        0x12 | 0xa4 | 0xa5 => "LeftAlt",
        0x5b | 0x5c => "LeftMeta",
        0x20 => "Space",
        0x0d => "Enter",
        0x08 => "Backspace",
        0x1b => "Escape",
        0x09 => "Tab",
        0x14 => "CapsLock",
        0x26 => "Up",
        0x28 => "Down",
        0x25 => "Left",
        0x27 => "Right",
        0xbc => "Comma",
        0xbe => "Period",
        0xbf => "Slash",
        0xdc => "Backslash",
        0xba => "Semicolon",
        0xde => "Quote",
        0xbd => "Minus",
        0xbb => "Equals",
        0xdb => "OpenBracket",
        0xdd => "CloseBracket",
        0xc0 => "BackQuote",
        _ => return None,
    };
    key_label(&format!("Vc{name}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fade_repress_order_and_cap() {
        let mut s = State::default();
        s.push("W".into(), true, 0.0);
        s.push("W".into(), false, 10.0);
        assert_eq!(s.snapshot(235.0), vec![("W".into(), 0.5)]);
        assert_eq!(s.snapshot(460.0), vec![("W".into(), 0.0)]);
        s.push("W".into(), true, 461.0);
        assert_eq!(s.snapshot(1000.0)[0].1, 1.0);
        s.push("W".into(), false, 1000.0);
        assert!(s.snapshot(1451.0).is_empty());
        for n in 0..11 {
            s.push(n.to_string(), true, 0.0);
        }
        assert_eq!(s.snapshot(0.0).len(), 10);
        assert_eq!(s.snapshot(0.0)[0].0, "1");
    }
    #[test]
    fn labels_and_queue() {
        assert_eq!(key_label("VcRightControl").as_deref(), Some("Ctrl"));
        assert_eq!(virtual_key_label(0x57).as_deref(), Some("W"));
        assert_eq!(virtual_key_label(0x126).as_deref(), Some("↑"));
        assert!(virtual_key_label(0x68).is_none()); // NumPad8.
        assert!(virtual_key_label(0x10d).is_none()); // NumPadEnter.
        assert_eq!(virtual_key_label(0xc0).as_deref(), Some("`"));
        assert_eq!(virtual_key_label(0x87).as_deref(), Some("F24"));
        assert_eq!(key_label("VcLeft").as_deref(), Some("←"));
        assert!(key_label("VcNumPad1").is_none());
        let q = Queue::new();
        q.push(87, true);
        assert!(q.pop().unwrap().down);
        assert!(q.pop().is_none());
    }
}
