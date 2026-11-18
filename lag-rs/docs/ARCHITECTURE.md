# lag-app architecture (Rust/Slint port of the Avalonia app)

Read with `docs/AGENTS.md`. Specs per area: `docs/spec/*.md`.

## Crates
| crate | owns |
|---|---|
| lag-ffi | raw FFmpeg/mpv bindings, `media::{EncodedPacket, StreamSpec}`, runtime DLL copying |
| lag-core | settings.json, clip metadata, i18n (17 langs), FileLog, presets, hardware info |
| lag-audio | WASAPI/process loopback, AAC tracks, noise gate, RNNoise, engine clock |
| lag-capture | the replay engine: WGC, NV12, encoder, rings, mux, carry-over, stalls, detect, overlay |
| lag-player | libmpv player rendering into an OpenGL texture shown by Slint |
| lag-app | the exe `Lag.exe`: Slint UI, view models, tray, hotkeys, toasts, HUD, updater |

## Threads
- **UI thread** = Slint event loop (winit). Owns every Slint component and every view model.
  Never block it: no disk scans, no FFmpeg, no mpv init, no engine calls that can wait on locks.
- **Background work**: `std::thread` (or a small pool) per job; results go back with
  `slint::invoke_from_event_loop` / `Weak::upgrade_in_event_loop`. View models live on the UI thread only
  (`Rc<RefCell<..>>`), so they never need locks.
- **Engine**: `lag_capture::engine::ReplayEngine` runs its own threads (lock timer, WGC callbacks, audio
  pump). The app talks to it through `RecorderService` (lag-app/src/services/recorder.rs), which calls the
  engine off the UI thread and forwards engine events (target changed, replay saved) to the UI.

## UI structure
- `ui/app.slint` exports `AppWindow` (the MainWindow): custom chrome (no OS frame, `no-frame: true`),
  top command bar, icon rail, content host switching between pages, capture flyout. 1:1 with
  `Views/MainWindow.axaml` (spec: shell.md §3.1).
- One `.slint` file per page under `ui/views/` (`library.slint`, `player.slint`, `editor.slint`,
  `settings.slint`) built from `ui/components` + `ui/theme` (design system, already done).
- **View-model bridge**: each page has an `export global <Name>VM { in-out property ...; callback ...; }`
  declared in its own .slint file; Rust module `src/views/<name>.rs` sets properties and binds callbacks.
  Lists use `ModelRc<VecModel<StructType>>`.
- **Localisation**: `export global Tr { in property <int> rev; pure callback t(string, int) -> string; }`.
  Markup writes `Tr.t("Btn_SaveReplay", Tr.rev)`; Rust implements `t` via `lag_core::i18n`, and bumps `rev`
  on language change so every string re-evaluates. Keys are the C# resource keys, unchanged.
- **Theme**: `Theme.mode` (dark/light/system) from `ui/theme/tokens.slint`.

## Windows that must NEVER take focus (the user plays games while Lag runs)
Toasts (`ui/toast.slint`), the stats HUD (`ui/hud.slint`) and any other overlay are separate Slint
windows. After the native window exists, Rust (src/shell/nofocus.rs) must:
1. get the HWND (Slint `raw-window-handle-06` feature / `window().window_handle()`),
2. set `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` (HUD also `WS_EX_TRANSPARENT | WS_EX_LAYERED`
   and `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` like C#),
3. show with `ShowWindow(SW_SHOWNOACTIVATE)` / `SetWindowPos(..., SWP_NOACTIVATE)`; never `SetForegroundWindow`.
A window that steals focus from a fullscreen game is a release-blocking bug.

## Shell services (lag-app/src/shell/)
tray (`tray-icon` + `muda`, menu items per shell.md §3.4), global hotkeys (WH_KEYBOARD_LL on its own
message-loop thread; callback only enqueues), toasts (queue, timings per shell.md §3.3/§4.6),
single instance, startup sequence (shell.md §4.1), updater (AppUpdateService), autostart registry.

## Player (lag-player)
libmpv with `vo=libmpv` render API (OpenGL). Slint femtovg renderer is OpenGL: in
`set_rendering_notifier(BeforeRendering)` render the mpv frame into an FBO texture and show it through
`slint::Image::from(BorrowedOpenGLTextureBuilder)`. mpv is created on a background thread at startup
(prewarm) so opening a clip never blocks the UI (fixes the 1.8.0 10-second freeze by design).
HW decode: `hwdec=auto-safe` (d3d11va via ANGLE/dxinterop where available, else copy-back).
