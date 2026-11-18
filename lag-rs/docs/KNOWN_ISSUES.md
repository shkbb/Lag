# Known issues (found by headless snapshots, 2026-11-16)

Check a page without a window: `target\debug\Lag.exe --snapshot <out.png> settings|library|player|editor [WxH]`
(software renderer in memory; tray, hotkeys and the engine are not started). Markup-only bisecting:
`target\debug\examples\slint_snap.exe <file.slint> <Component> <out.png> [WxH]` (Slint interpreter).

## Slint sizing rule (caused the "blank window" bug)
An element NOT inside a layout and without an explicit width/height does NOT fill its parent: it takes
its children's preferred size (often 0). Every top-level container, page root, `PageLayer`, `FocusScope`
and custom component like `LagScrollView` must get `width: parent.width; height: parent.height` (or
`100%`) at the USE site. Component roots cannot reference `parent`.

## Open
1. Player and Editor clip lists are empty ("Кліпи: 0") — not fed from the library scan (C#: Player/Editor
   use the library's clips, MainViewModel.EditableClips). Wire through AppCtx (library publishes its list).
2. Player: the big play button overlaps the "Відеопрогравач" placeholder text.
3. FIXED (main.rs SetProcessDpiAwarenessContext PerMonitorV2). Was: no DPI awareness: monitors are reported in logical pixels (3440x1440 @125% shows as 2752x1152).
   Needs an app manifest (PerMonitorV2) embedded in Lag.exe like C# app.manifest, or
   SetProcessDpiAwarenessContext at startup before any window/monitor query.
4. Monitor names show `\\.\DISPLAY1 (…) [Pri…]` — compare with the C# display label format.
5. Rail label "Налаштування" wraps mid-word ("Налаштува / ння"); C# had MaxWidth 56, 9.5px, 2 lines.
6. HUD needs a frames-captured counter from the recorder service (shell-B open item).
7. Codex packages stopped by usage limit: shell-A, shell-B, settings, player need a pass against
   their spec §10 checklists.
8. Engine live tests still missing: screen→game return in "record the desktop" mode; game exit →
   desktop continuation; overlays on a real game.
9. Player fullscreen (MainVM.is-fullscreen + window.set_fullscreen) shows NOTHING at all (owner, 17:20,
   texture player). Player v2 must re-test fullscreen explicitly: native mpv surface covering the monitor,
   overlay controls on top, Escape leaves, chrome restored.
10. Texture player: stripes on the FIRST play of a clip, gone after pause+resume (d3d11va-copy path).
    Superseded by player v2 (zero-copy d3d11va in a native mpv window). The EDITOR still uses the texture
    path: check it for the same stripes; if present, try `hwdec=no` / `d3d11va` + interop there.
11. FIXED 2026-11-16: Settings > Audio per-app rows: icons (exe icon / Material icons), left names (checkbox
    no longer stretches), % and 'Auto' vertically centred, hint styled like C#, PTT chip sized like C#.
12. Settings > General: language picker flag misaligned vertically with its text.
13. FIXED 2026-11-16: player seek bar fill grew from the CENTRE (fill Rectangle had no `x`). Slint rule: a
    child with no `x`/`y` outside a layout is CENTRED in its parent (Avalonia/XAML put it top-left). Every
    progress/fill/meter child must set `x: 0; y: 0;` explicitly. Scanner: scratch xscan2.py idea — flag any
    element with a non-full `width` and no `x` whose parent is not a layout; run it before handing over UI.
14. FIXED 2026-11-16 (found by ui-check): library context menu stretched to page height; card time/game line
    drawn a row lower; editor transport overlap at min width; status pill not re-translated; filter names
    hard-coded English (C# too) -> Filter_* keys; 14 keys untranslated in 15 languages (C# too); library dates
    used the Windows locale instead of the app language (C# too); Japanese date order.
15. DECIDED + FIXED 2026-11-16 (owner: "switch to the Windows default devices"): mic and webcam lists start
    with "Default" (empty id = follow Windows). An absent saved device falls back to it (C# took the first
    device). Recorder maps empty/absent -> None = Windows default; Some("") still means "no mic".
16. NOT headless-checkable (verify with owner): player with a clip + its controls (player v2), real fullscreen,
    dropdown animation smoothness, library scroll smoothness, GPU video quality.
17. FIXED 2026-11-16 (player v2 review): overlay seek-bar fill centred again (copied pre-fix) + 280px rows;
    overlay controls never appeared on hover (`moved` needs a held button) -> pointer-event move, same in
    player.slint and ReplayListItem; 33 ms overlay sync ran even in the tray -> skipped when not visible;
    rounded corners never applied with "start minimized" (no HWND at startup) -> re-applied until it sticks
    and on every show; "EDITED" badge translated. Live-only checks remain: video quality/fps
    ([PlayerStats] lines in the log, 6 s into each clip), fullscreen drawing, overlay focus behaviour.
18. 2026-11-16 owner test of player v2: (a) crackling/warbling audio = mpv video-sync=display-resample on VFR
    clips resampling AUDIO (recorded audio verified clean: 0 gaps/clicks in all 3 tracks) -> 1x uses
    video-sync=audio; slow motion (<1x) switches to display-resample + interpolation (tscale=oversample) for
    smooth slow-mo. (b) fullscreen warp: video re-shown on the first resize step -> hidden until size stable
    150 ms. (c) volume slider and speed chip top-aligned in the 38 px row -> centred. (d) keys overlay "random"
    keys: injected events (LLKHF_INJECTED) now ignored, hook thread TIME_CRITICAL, held keys reconciled with
    GetAsyncKeyState every 500 ms. (e) editor: hwdec was set to "no" (CPU decode of 2580x1080@165 = slideshow,
    commands queued behind decoding -> pause/stop "dead") -> d3d11va-copy; pause always sent on page leave.
    OPEN: editor needs the player's native-surface engine (overlay window for crop handles/captions).
19. 2026-11-16: exit with the engine running left a half-dead process in the NVIDIA driver holding the WGC
    capture (yellow screen border, needed a reboot). Exit now: recorder.shutdown_blocking(3 s) -> services ->
    settings -> log -> TerminateProcess. Measured engine stop while encoding: 24 ms.
20. 2026-11-16 round 3 (owner): fullscreen now fades video to black (mpv brightness, ~120 ms) -> switch ->
    shown when size stable -> fades in (~180 ms); slow-mo tscale=linear (oversample only blended one vsync);
    overlay right row 34 px = shared centre line (measured); capture flyout cut out of the native video via
    SetWindowRgn (MainVM.occluder-changed); combobox + language picker open one frame after the popup appears
    (set in `init` = no animation, verified frame by frame); flags centred; SegmentedControl hugs its height;
    game names normalized (Minecraft titles -> "Minecraft", trailing '*') at record AND library read time.
    OPEN: library game-filter chip row clips the last chip (no horizontal scroll/fade); one lag-capture GPU
    test failed once while Lag was recording in parallel (5 reruns green) - suspected GPU contention.
21. 2026-11-16 round 4: (a) CRACKLE ROOT CAUSE: apps-mode mixer took a fixed 20 ms per 20 ms tick; capture and
    tick jitter left queues a few samples short -> ~80-sample zero holes in the system track (verified on the
    owner's clip: flat zero plateaus) + cumulative audio delay. New AppMixer (30 ms jitter buffer per source,
    wall-clock pacer) + unit tests. Needs a live clip check (scratch crackle.py). (b) "Now recording" toast only
    on game enter/exit/change, not alt-tab (game process still running). (c) speed menu: "Smooth slow motion"
    toggle, persisted (SlowmoInterpolation, Rust-only key). (d) fullscreen: full-monitor black layered cover
    (shell/fade_cover.rs) instead of video brightness. (e) startup splash (ui/splash.slint, app colours) on
    manual launch, >= 1.4 s. (f) themed tray menu (ui/tray_menu.slint) instead of the native one.
    OPEN: settings "grid shifts a few px when a dropdown opens" - not reproducible headless (100 % and 125 %
    scale, pixel diff = only the list changes); needs a live-window look.
22. 2026-11-17: (a) fullscreen "less smooth": mpv profile=high-quality's ewa_lanczossharp upscaler dropped frames
    at 3440x1440/165 Hz (owner log: 42 drops; hidden-window probe at 3440x1440: 34 drops/15 s) -> scale=lanczos
    (0-3). (b) settings "grid shifts ~1 px while a dropdown is open": measured frame by frame on the owner's
    video (whole left column -1,+1 px only while open). Not reproducible with the software renderer; Slint
    PopupWindow path in the GPU renderer. Both PopupWindows removed: one window-level DropdownLayer +
    DropdownHost global (components/combobox.slint) draws every list. Needs live confirmation.
    (c) frameless window resize: 8 px DPI-scaled grab border via WM_NCHITTEST subclass (main_vm.rs).
23. FIXED 2026-11-17 (verified on real-window GPU snapshots): settings "grid shifts / comes alive" = any
    transform-scale/rotation (dropdown chevron rotated 180 while open, press 0.95/0.97, checkbox pop, slider
    hover) made FemtoVG redraw the whole card/page shifted at 125 %. All transforms removed (chevron by
    computed path, thumbs grow by size). Before: open list changed x 421..1376; pressed "165" changed the whole
    page. After: only the list / nothing outside.
24. 2026-11-17: player keys (Space/Esc/M/arrows) did nothing -> handled at window level (winit event, player page)
    + keyboard focus pulled back to the main window when one of our other windows (native surface, mpv child,
    overlay) holds it; focus owner changes logged ("[Player] keyboard focus on window class ..").
    Fullscreen: overlay window hidden while playing with no controls shown (mouse over the video re-shows it,
    cursor auto-hide moved to the main window); [PlayerStats] now also logs DWM composition missed/dropped.
25. 2026-11-17 (owner test of 24): keys work, but (a) a click on the fullscreen video no longer paused -> the main
    window's TouchArea toggles in fullscreen (the overlay is hidden there). (b) cursor must hide together with the
    controls in window AND fullscreen: overlay idle Timer (2.5 s, playing, not pinned) hides controls/sidebar and
    the cursor (PlayerOverlayVM.cursor-idle; main window follows via PlayerVM.cursor-hidden). Slint: restart() does
    not start a stopped Timer -> `running = false; running = true`. (c) toasts not at the screen edge: the window
    is sized to the card (min(350, card)) instead of a fixed 350 px with the card centred. (d) fullscreen bottom
    bar sometimes flew in from the top: only a slide offset is animated now (animating y itself replayed from the
    old position when the overlay window was re-shown/resized). (e) DWM counter removed from [PlayerStats]
    (Win11 reports composed=1 per 3 s: meaningless). Needs live confirmation.
26. 2026-11-17 (owner test of 25: toasts OK, click-pause OK): (a) cursor never hid: Slint applies `mouse-cursor`
    only while it handles a pointer event, so `none` set by an idle timer (no movement by definition) never
    reached the screen -> player.rs hide_cursor_over: while idle+playing and the cursor is over our video
    window, SetCursor(NULL) every tick. Windows' fake WM_MOUSEMOVE (a window under a still cursor shown/hidden)
    no longer wakes the controls: synthetic-move() compares GetCursorPos with the last event (off for
    --ui-check/--gpu-check). (b) panels faded in but vanished instantly: `visible: shown` -> `opacity > 0`, and
    in fullscreen the overlay window hides 300 ms after the controls (after the 250 ms fade). (c) fullscreen
    1-2 px strip around the screen: Win11 kept round corners + 1 px DWM border -> DWMWCP_DONOTROUND +
    DWMWA_BORDER_COLOR none while fullscreen, restored when leaving. (d) F (and "а" on the UA layout) toggles
    fullscreen on the player page when a clip is loaded. Needs live confirmation.
27. 2026-11-17 (owner test of 26): (a) fullscreen cursor did not hide at first, then flickered after a theme
    switch: the hide depended on a main-window Slint timer started only by a pointer event there (filtered as
    synthetic after 26) -> idle measured in the player tick from GetCursorPos (2.5 s, both modes); the flicker
    was winit answering every WM_SETCURSOR (repaints too, not only movement) with the arrow -> cursor_proc
    subclass on the main + overlay windows answers "no cursor" while hidden and the cursor has not moved.
    (b) light theme: thumbnail chips (Edit, favourite, selection circle, duration) were dark with dark text
    (same in C#) -> cream glass chips + Theme.card-badge-fg. (c) player: the square native video window stuck
    out of the 12 px rounded frame (seen on the light theme) -> surface clipped with CreateRoundRectRgn (11 px
    inner radius x DPI), square in fullscreen. New check script: tools/ui-check/light.txt.
28. 2026-11-17 EDITOR (owner: "start on the editor, fix the layout, check everything"):
    (a) layout: the timeline had no height limit and took half of the centre column (video shrank to a strip,
    filmstrip glued to its bottom) -> height = content; preview min 180 px (C#). (b) audio lanes never drew
    (EditorAudioLane's inner row had no size -> 0x0; space was reserved, nothing painted) -> sized.
    (c) ENGINE: preview moved from the GL-texture path (mpv render API -> Slint image, d3d11va-copy) to the
    player v2 engine: native child window (mpv wid, gpu-next, d3d11, hwdec=d3d11va; -copy only while a vf
    chain is active; same vf is never re-set) placed at the video rect, clipped to the stage (top corners
    rounded, capture flyout cut out), keepaspect=no (reframe stretch). Everything drawn over the video moved to
    EditorStageOverlay, shown in EditorOverlayWindow (transparent, non-activating, owned) over the stage;
    callbacks forward to EditorVM. Headless checks keep the in-scene fallback. mpv is created on first editor
    visit only (was prewarmed at startup). (d) light theme: transport bar = player bar colours, play button
    accent, lane chip text light, lane handles bronze (C# had a dark bar with invisible icons there).
    New: tools/ui-check/editor.txt (`editor-clip N`), tools/ui-check/gpu-editor.txt (`screen` grabs the real
    screen incl. native windows). Needs: a live gpu-check run + owner test.
29. 2026-11-17 (owner): (a) tray menu opened and closed at once, the main window jumped forward: the player's
    keep_keyboard_focus pulled focus from ANY other window of ours (the themed tray menu is one) -> only from
    the main window's children and the player overlay. (b) app icon: Lag.exe now embeds Assets/icon.ico
    (build.rs -> windres COFF object; the taskbar showed a blank document); window icon + tray icon follow the
    theme (assets/icon-dark.png = original, icon-light.png = cream + bronze, generated by recolouring the
    original's pixels). (c) light theme colour mismatches: lane handles/chips, timeline/thumbnail placeholders,
    filter hover, caption selection, disabled slider fill (was full cyan in BOTH themes -> 35 % accent),
    caution/extreme segment text (yellow "120" unreadable on cream). Segmented control: the sliding pill now
    also places itself once after the first layout (a first value fires no `changed`).
30. 2026-11-17 owner test of the native editor ("not bad overall"): (a) play button once did nothing (worked
    after re-entering the page): cause unknown -> play/pause presses and mpv pause/eof/load are now logged
    ([Editor] ...). (b) crop cursors: hover used `moved` (needs a held button) -> pointer-event move; resize
    arrows per edge/corner, kept while dragging; move cursor inside the box. (c) overlay window was square and
    drew shades/handles over the card's corners + border -> SetWindowRgn = stage card shape. (d) audio follows the
    video trim by default ("Audio follows video" chip in the timeline header turns it off). (e) captions are edited
    in place: double-click -> text field on the video (overlay made activatable meanwhile via
    nofocus::set_activatable; Enter/Esc/click elsewhere ends); caption rows now update in place. (f) preview speed:
    colour presets + sliders = one mpv user shader driven by glsl-shader-opts (no filter/decoder restart), crop =
    video-crop, rotation = video-rotate; only flips stay a CPU vf. Export unchanged (exact ffmpeg filters).
    (g) stats HUD always "FPS --": reads the engine's frame counter directly now (C#-like), logged every 10 s.
    (h) tray/window icons from the .ico's own hand-drawn sizes (a 256 px image scaled to 32 made the arc too thin);
    icon-light.ico = every size recoloured.
31. 2026-11-17 (owner, angry - rightly): (a) PLAY after re-picking a clip did nothing - proven by the new log:
    load -> paused=false -> END_FILE of the PREVIOUS file arrived last and set "not playing" while mpv played;
    the press then sent "play" to a playing mpv. Stopped is ignored now; after Play/Pause/Load the worker reports
    mpv's real `pause` (UI can no longer drift). (b) captions: all new texts stacked on one spot (looked like
    "only one text") -> cascaded; picking a text ends crop/reframe selection; a click on the video deselects the
    text; drag on pointer events; double-click edits ON the video (caret + live text) while the Text-tab field
    holds the keyboard (no overlay activation hack any more). (c) overlay settings: rows with their own switch
    dimmed label + switch when off -> only the dropdown dims; switches centred. Stats HUD now needs
    overlay_enabled && stats_shown (C#). (d) HUD "FPS --": frames=0 in the log = no recording running; FPS is the
    recording's capture rate (C# too). New: gpu-check `ov-*`/`type`/`key`/`editor-addtext`, gpu-editor2.txt.
32. 2026-11-17 live gpu-check (owner allowed): play after re-picking a clip toggles correctly (log: playing=true
    -> pause -> play); captions: 3 added cascade, crop tool ends on text click, drag moves, double-click + typing
    edits on the video, Enter ends. Theme cross-fade measured on screen: light 234 -> 191 at 120 ms -> 25 at
    270 ms -> 12 (dark). "Audio follows video" renamed "Link audio" / "Прив'язати звук" (owner: awful name).
33. 2026-11-17 (owner): (a) "stripes along the video edges" = the always-on frame outline -> shown only while
    cropping / the reframe box is selected. (b) one left click both selected the video and paused it -> click =
    play/pause; drag = move (selects); click outside the selected box = deselect only; clicks never pause while
    cropping. Checked headless (tools/ui-check/editor-click.txt).
34. 2026-11-17 (owner): (a) the editor overlay window grew a classic title bar "Lag Editor Overlay" + X: a
    SetWindowRgn on that frameless winit window -> removed; the card shape is clipped in Slint (top corners
    13 px, 1 px border inset). (b) tray/window icon stayed light with "System": the theme was read at startup
    before the OS scheme was known -> 1 s watcher keeps both icons on the resolved theme. (c) "System theme does
    not save": MY launch script forced Theme=light into the test settings copy before every run - stopped.
    (d) "click/stripes unchanged": the owner tested the previous (20:46) build; the fix ships with this one.
35. 2026-11-17 SETTINGS AUDIT (owner away, PC at my disposal). Method: every row traced UI -> SettingsData ->
    consumer, compared with C# SettingsViewModel; then REAL recordings through the Settings code paths in a
    separate APPDATA copy (library = scratch folder) via new gpu-check commands (rec/save/choose/toggle/number):
    A (owner's settings): 2580x1080 (1440 monitor -> 1080p), mp4, 3 audio tracks (All default + System + Mic),
      duration right. B: mkv + hevc + 720p (1720x720) + 60 fps grid + 10 Mbps CBR (10.3) + 30 s buffer (38 s
      recorded -> 30.2 s) + no separate tracks/no mic -> 1 track. C: keys overlay baked in (frame checked); with a
      game running the engine locks onto the game window (monitor choice = desktop only, as C#). D: PTT fix.
    FIXED: (1) PTT turned off during a recording left the mic muted until restart (C# too; the old unmute call
    read the already-changed setting) -> live unmute/mute on PTT switch (verified: "[PTT] switched off during
    recording -> mic live"). (2) library folder change while recording gave no "applies after restart" note
    (session keeps saving into the old folder; C# marked it restart-required) -> marked. (3) MKV video track had
    default=0 -> default. Earlier today: overlay rows dimming, stats HUD gated by the overlay switch, FPS counter.
    NOTED, unchanged (= C#): track titles not stored in mp4; fixed-bitrate fps-adapt resets the buffer once early in
    a session; target\release lacks rnnoise\std.rnnn (run-v2 has it) -> installer must ship rnnoise/.
    Not exercised live (privacy / OS input): webcam overlay, hotkey capture, check-updates (GitHub), open-folder.
36. 2026-11-17/18 audit follow-up (owner's asks):
    (a) TRACK NAMES: mp4 stored them only in udta `name`; players read the `hdlr` name ("SoundHandler") -> both
      `title` and `handler_name` set; names "All audio" / "System audio" / "Microphone" ("Audio" single track).
      Verified with ffprobe on a real recording.
    (b) RELEASE FILES: lag-ffi build.rs now mirrors InstantReplay\ffmpeg\ and \rnnoise\ into target\<profile>
      (a plain cargo build folder is complete); tools\package.ps1 assembles the installer folder and FAILS on any
      missing runtime file (exe, FFmpeg/libmpv DLLs, ffmpeg\ffmpeg.exe+ffprobe.exe, rnnoise\std.rnnn, Assets);
      installer\Lag.iss documents it. MicDenoiser logs "model missing" instead of a bare setup failure.
      Verified: "[MicDenoiser] arnndn (RNNoise) graph ready" from target\release.
    (c) WEBCAM, live (Logitech StreamCam): overlay baked into the recording, rounded box, real picture. C# and the
      port picked the LOWEST rate >= 15 -> streamed 640x480@15 -> Auto now = ~640 wide @ ~30 fps. NEW setting
      "Webcam quality" (Overlay tab, `WebcamMode`, Rust-only key at the end of settings.json): Auto,
      640x480/30, 1280x720/30, 1280x720/60, 1920x1080/30, 1920x1080/60; the camera's nearest mode is used when
      the exact one is missing (unit test mode_choice), MJPG modes are decoded to BGRA by the frame reader.
    (d) HOTKEYS, live with REAL injected keys (F13-F24 + Ctrl only; gpu-check `hotkeys` / `action` / `sendkey`):
      BUG FOUND: recording a new hotkey in Settings never saw the key. winit registers the process for keyboard
      raw input, and Windows then skips the process's own WH_KEYBOARD_LL hook while one of its windows is the
      foreground (PowerShell repro: RegisterRawInputDevices(keyboard) on its own window -> 0 hook calls). In a
      game (Lag not focused) PTT still worked, which is why nobody noticed. Fix: the hotkey service drops the
      keyboard raw-input registration (winit only uses it for DeviceEvent::Key, Slint never reads those).
      After the fix, end to end: captured Ctrl+F21 / F22 / F23 / F24 / PTT F20 into settings; F24 started and
      stopped recording, PTT held/released, F23 paused 1.6 s and resumed, F22 wrote a PNG, bare F21 did nothing,
      Ctrl+F21 saved the replay. (Alt+F7..F10 were taken by the owner's running Lag: error 1409, expected.)
      LAG_HOTKEY_TRACE=1 logs every key the hook delivers.
    (e) EDITOR IMPORT: "Import" button in the editor header opens any video (mp4/mkv/mov/webm/avi/m4v/wmv/flv/
      ts/mts/m2ts) through the system Open dialog; export writes `<name>-edit.<ext>` next to that file.
    (f) FLAKY: `cargo test -p lag-capture --lib` crashes ~1 in 5 runs with STATUS_ACCESS_VIOLATION when the whole
      suite runs in parallel; every group alone and the full suite with --test-threads=1 pass. Driver-level race
      between tests that open D3D/encoders/cameras at the same time (investigating which pair).
37. 2026-11-18 (owner): (a) 9:16 "works like a cripple": the native preview surface was clipped to the whole
    stage, so a 9:16 frame over a 21:9 clip still showed the wide picture -> clipped to the export frame unless the
    frame is being edited (crop tool / reframe box selected: whole stage + dimmed outside). Export presets scaled
    by height for every shape (9:16 "1080p" = 608x1080) -> the preset is the SHORT side (1080x1920).
    Tests: preview_shows_only_the_export_frame, vertical_export_scales_the_short_side.
    (b) CLIP MERGING: editor sidebar "Merge" -> pick clips in order (numbered cards) -> "Merge (N)" ->
    services/clip_merge.rs: same codec/size/audio layout = concat demuxer stream copy (two Lag recordings: 0.5 s,
    all 3 named tracks kept); otherwise one re-encode (letterboxed to the first clip's size, first audio track,
    silence for clips without sound, NVENC then x264). Result `<first>-merged.mp4` opens in the editor.
    Verified with real files (copy + 2 re-encode cases) and headless ui-check tools/ui-check/editor-merge.txt.
    (c) Library thumbnails were squashed for non-16:9 clips (`-s 320x180`, C# too) -> generated into a 384x216
    box keeping the shape, cards crop (cover); existing 320x180 thumbnails are regenerated once.
    (d) Code quality pass: FFmpeg helpers moved out of views/editor.rs into services/ffmpeg.rs (the library had
    its own copy of the tool lookup); merge UI in views/editor/merge.rs; dead code from the old GL editor path
    removed (render hooks, GlStateGuard, glow dependency, unused Brightness command, focusable-overlay switch,
    hotkey suppress switch, unused fields). lag-app now builds with 0 compiler warnings.
38. 2026-11-18 EDITOR REFACTOR (owner: "code must be written properly, OOP"). views/editor.rs (4169 lines, one
    1200-line init) split, code moved verbatim by a boundary-checked script, then tidied:
      editor.rs (root, ~95 lines: module map + init orchestration)
      editor/model.rs      EditModel and its data types (pure, no UI/FFmpeg)
      editor/geometry.rs   crop/reframe transforms and drag rules
      editor/preview.rs    mpv look: shader params, audio routing, flips
      editor/export.rs     FFmpeg filter graphs/args + export worker
      editor/import.rs     Open dialog
      editor/worker.rs     mpv control thread, probe, timeline/filter thumbnails (UiMessage)
      editor/runtime.rs    EditorRuntime (UI-thread state, new(), sync_* to EditorVM), static options
      editor/bind_*.rs     button wiring by area: navigation (+ app events), timeline, frame, text, export
      editor/ui_loop.rs    native host + 33 ms tick
      editor/native.rs     (was views/editor_native.rs) native video window + overlay
      editor/merge.rs      clip merging UI;  editor/tests.rs  the editor's unit tests
    services/ffmpeg.rs (shared FFmpeg/FFprobe runner, probe, encoder args, output naming) and
    services/clip_merge.rs are used by the editor and the library. rustfmt.toml (max_width 120) + `cargo fmt --all`
    over the workspace (0 diffs left). 0 compiler warnings. All tests green; headless ui-check before/after: only
    expected differences (aspect-correct thumbnails, disk usage text).
    Still large (next): library.rs 2.5k, settings.rs 2.5k, player.rs 2k lines; 18 clippy notes in lag-app.
