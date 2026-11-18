# Player v2: native mpv video surface (decided 2026-11-16)

Goal (owner): playback must look AT LEAST as good as the Windows player: full source resolution/bitrate,
no blurry scaling, every frame at its time on a 165 Hz monitor. The C# libVLC player and our first Slint
texture player both fell short (UI-side bilinear scaling, GPU->RAM->GPU copies, frame pacing tied to the UI
render loop, plus GL-state bugs: upside-down/striped frames).

## Architecture
- The PLAYER page shows video in a native child window (WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
  no input: WS_EX_TRANSPARENT-style hit-testing via WM_NCHITTEST -> HTTRANSPARENT so clicks reach Slint) owned
  by the main Slint HWND. mpv renders straight into it: `wid=<hwnd>`.
- mpv options: `vo=gpu-next`, `gpu-api=d3d11`, `hwdec=d3d11va` (zero-copy), `profile=high-quality`
  (`scale=ewa_lanczossharp`, `dscale=mitchell`, `cscale` default of the profile), `video-sync=display-resample`,
  `interpolation=no` (replays are VFR game footage: show real frames, never blend), `keep-open=yes`,
  `osc=no`, `osd-level=0`, `input-default-bindings=no`, `input-vo-keyboard=no`, `cursor-autohide=no`,
  `background=color` + `background-color=#000000`, `hwdec-codecs=all`. Colour: honour file tags (BT.709
  limited, as the engine writes); `target-colorspace-hint=no`.
- Geometry: the Slint player page exposes the video rectangle (logical px via absolute-position + size).
  Rust converts to physical px (window scale factor) and `SetWindowPos`es the child (SWP_NOACTIVATE |
  SWP_NOZORDER); on every layout change, window resize, DPI change, fullscreen toggle. Hidden (SW_HIDE) when
  the page is not visible, the window is minimised, or a modal/popup must cover the video.
- Overlays ABOVE the video (control bar, big play button, speed/audio menus) cannot be drawn by the parent
  window (airspace). They live in ONE small owned top-level Slint window: WS_POPUP, WS_EX_LAYERED |
  WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, transparent background, positioned over the video rect, shown on
  mouse movement and auto-hidden like the C# bar. It must never steal focus (shell::nofocus rules), and it is
  hidden when the main window is minimised or another page is shown.
- One mpv instance for the player. Control from a worker thread; events (time-pos, duration, pause, eof,
  file-loaded, track-list) observed and forwarded to the UI thread. Switching clips: `loadfile <path>
  replace` + explicit state reset (position, duration, playing) in the UI; never two instances playing.
- The EDITOR keeps the OpenGL texture path for its preview (it needs crop handles and captions drawn on
  top of the video), with the GL-state guard (pixel-store reset) and lazy creation; its quality target is
  "editing preview", the PLAYER's is "best possible".

## Acceptance
- Frame-accurate: a 165 fps clip plays without dropped/duplicated frames (mpv `frame-drop-count` and
  `vo-delayed-frame-count` stay 0 over a 2-minute clip at 1:1 window size).
- Sharpness: side-by-side screenshots vs. the Windows player show no softer detail.
- Switching between 10 clips quickly: always the clicked clip's video AND audio, state UI in sync.
- No focus is ever taken from a game by the player overlay window.
